use anyhow::Result;
use log::{error, info};
use mongodb::{
    bson::{doc, DateTime as BsonDateTime, Document},
    Client as MongoClient, Collection,
};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use uuid::Uuid;

use chrono::{DateTime as ChronoDateTime, Utc};

#[derive(Debug, Deserialize, Clone)]
struct Question {
    id: String,
    title: String,
    #[serde(rename = "resolveBy")]
    resolve_by: Option<ChronoDateTime<Utc>>,
    #[serde(rename = "resolvedAt")]
    resolved_at: Option<ChronoDateTime<Utc>>,
    #[serde(default)]
    resolved: bool, // Fatebook returns this; default false if missing
}

#[derive(Debug, Deserialize)]
struct GetQuestionsResponse {
    items: Vec<Question>,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    // Default log level = info, overridable via RUST_LOG
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .init();

    info!("service starting up");

    loop {
        let started = Utc::now();
        if let Err(e) = run_once().await {
            error!("run_once failed: {:?}", e);
        }
        let ended = Utc::now();
        info!("run finished in {}s", (ended - started).num_seconds());

        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}

async fn run_once() -> Result<()> {
    let fatebook_api_key = env::var("FATEBOOK_API_KEY")?;
    let beeminder_auth = env::var("BEEMINDER_AUTH_TOKEN")?;
    let beeminder_user = env::var("BEEMINDER_USER")?;
    let beeminder_goal = env::var("BEEMINDER_GOAL_SLUG")?;
    let mongo_uri = env::var("MONGODB_URI")?;
    let mongo_dbname = env::var("MONGODB_DBNAME").unwrap_or_else(|_| "bridge".into());
    let processed_coll_name =
        env::var("MONGODB_COLLECTION").unwrap_or_else(|_| "processed_questions".into());

    let mongo_client = MongoClient::with_uri_str(&mongo_uri).await?;
    let db = mongo_client.database(&mongo_dbname);
    let processed_coll: Collection<Document> = db.collection(&processed_coll_name);

    let http = Client::new();

    // 1) unresolved + readyToResolve (due today / soon)
    let unresolved = http
        .get("https://fatebook.io/api/v0/getQuestions")
        .query(&[
            ("apiKey", fatebook_api_key.as_str()),
            ("unresolved", "true"),
            ("readyToResolve", "true"),
        ])
        .send()
        .await?
        .error_for_status()?
        .json::<GetQuestionsResponse>()
        .await?
        .items;

    // 2) resolved (newly resolved things we might need to count)
    let resolved = http
        .get("https://fatebook.io/api/v0/getQuestions")
        .query(&[
            ("apiKey", fatebook_api_key.as_str()),
            ("resolved", "true"),
            // Optionally: limit and/or sortEarliestFirst=true
        ])
        .send()
        .await?
        .error_for_status()?
        .json::<GetQuestionsResponse>()
        .await?
        .items;

    // Merge by id (resolved side overwrites unresolved if both present)
    let mut map: HashMap<String, Question> = HashMap::new();
    for q in unresolved.into_iter() {
        map.insert(q.id.clone(), q);
    }
    for q in resolved.into_iter() {
        map.insert(q.id.clone(), q);
    }

    info!("merged {} questions (unresolved+resolved)", map.len());

    let mut to_process = 0usize;
    let mut processed = 0usize;
    let mut successes = 0usize;
    let mut failures = 0usize;
    let now = Utc::now();

    for (_, q) in map.into_iter() {
        // Skip if no deadline — nothing sensible to compare against
        let Some(deadline) = q.resolve_by else {
            info!("{} has no resolveBy; skipping", q.id);
            continue;
        };

        // Have we already finalized this one in our DB?
        let already = processed_coll
            .find_one(doc! { "question_id": &q.id })
            .await?;
        if already.is_some() {
            // Only skip if it's been resolved OR deadline has passed.
            // If neither is true, don't mark as processed in DB (so we'll recheck later).
            info!("Now: {:?}, Deadline: {:?}", now, deadline);
            if q.resolved || now >= deadline {
                info!("skip {} (already processed & finalized)", q.id);
                continue;
            } else {
                info!(
                    "{} was seen before but not finalized; rechecking in future runs",
                    q.id
                );
                continue; // not finalized; but do not process again now (since we don’t double-post)
            }
        }

        to_process += 1;

        if q.resolved {
            // Final outcome known — success if resolved_at <= deadline
            let success = check_completion_logic(&q).await;
            if success {
                post_beeminder_datapoint(
                    &http,
                    &beeminder_user,
                    &beeminder_goal,
                    &beeminder_auth,
                    1.0,
                    &format!("SUCCESS (resolved): {}", &q.title),
                )
                .await?;
                successes += 1;
            } else {
                // If you prefer to skip failures, comment out this post
                post_beeminder_datapoint(
                    &http,
                    &beeminder_user,
                    &beeminder_goal,
                    &beeminder_auth,
                    0.0,
                    &format!("FAILED (resolved after deadline): {}", &q.title),
                )
                .await?;
                failures += 1;
            }

            processed_coll
                .insert_one(doc! {
                    "question_id": &q.id,
                    "title": &q.title,
                    "finalized_reason": "resolved",
                    "processed_at": BsonDateTime::now()
                })
                .await?;
            processed += 1;
            continue;
        }

        // Not resolved yet
        if now >= deadline {
            // Deadline passed → treat as failure
            post_beeminder_datapoint(
                &http,
                &beeminder_user,
                &beeminder_goal,
                &beeminder_auth,
                0.0,
                &format!("FAILED (deadline passed): {}", &q.title),
            )
            .await?;
            failures += 1;

            processed_coll
                .insert_one(doc! {
                    "question_id": &q.id,
                    "title": &q.title,
                    "finalized_reason": "deadline_passed",
                    "processed_at": BsonDateTime::now()
                })
                .await?;
            processed += 1;
        } else {
            // Not resolved and not due yet — do not mark processed; we’ll check again later
            info!(
                "{} not due yet (deadline {}), leaving unprocessed",
                q.id, deadline
            );
        }
    }

    info!(
        "summary: candidates={}, finalized={}, successes={}, failures={}",
        to_process, processed, successes, failures
    );

    Ok(())
}

async fn check_completion_logic(q: &Question) -> bool {
    match (q.resolved_at, q.resolve_by) {
        (Some(resolved_at), Some(resolve_by)) => resolved_at <= resolve_by,
        _ => false,
    }
}

async fn post_beeminder_datapoint(
    http: &Client,
    user: &str,
    goal: &str,
    auth_token: &str,
    value: f64,
    comment: &str,
) -> Result<()> {
    let url = format!(
        "https://www.beeminder.com/api/v1/users/{}/goals/{}/datapoints.json",
        user, goal
    );
    let request_id = Uuid::new_v4().to_string();

    let params = [
        ("auth_token", auth_token),
        ("value", &value.to_string()),
        ("comment", comment),
        ("requestid", &request_id),
    ];

    let resp = http.post(&url).form(&params).send().await?;
    let status = resp.status();
    if status.is_success() {
        info!(
            "beeminder POST ok (value={}, requestid={})",
            value, request_id
        );
    } else {
        let body = resp.text().await.unwrap_or_else(|_| "<no body>".into());
        error!("beeminder POST failed {} => {}", status, body);
    }
    Ok(())
}
