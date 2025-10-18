use anyhow::Result;
use log::{error, info};
use mongodb::{
    bson::{doc, DateTime as BsonDateTime, Document},
    Client as MongoClient, Collection,
};
use reqwest::Client;
use serde::Deserialize;
use std::env;
use uuid::Uuid;

use chrono::{DateTime as ChronoDateTime, Utc};

#[derive(Debug, Deserialize)]
struct Question {
    id: String,
    title: String,
    #[serde(rename = "resolveBy")]
    resolve_by: Option<ChronoDateTime<Utc>>,
    #[serde(rename = "resolvedAt")]
    resolved_at: Option<ChronoDateTime<Utc>>,
    // You can add more fields as needed (e.g., `resolved: bool`)
}

#[derive(Debug, Deserialize)]
struct GetQuestionsResponse {
    items: Vec<Question>,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    env_logger::init();

    loop {
        if let Err(e) = run_once().await {
            error!("Run failed: {:?}", e);
        }
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

    info!("Connecting to MongoDB at {}", &mongo_uri);
    let mongo_client = MongoClient::with_uri_str(&mongo_uri).await?;
    let db = mongo_client.database(&mongo_dbname);
    let processed_coll: Collection<Document> = db.collection(&processed_coll_name);

    let http = Client::new();

    info!("Fetching questions ready to resolve from Fatebook");
    let resp = http
        .get("https://fatebook.io/api/v0/getQuestions")
        .query(&[
            ("apiKey", fatebook_api_key.as_str()),
            ("readyToResolve", "true"),
            ("unresolved", "true"),
        ])
        .send()
        .await?
        .error_for_status()?
        .json::<GetQuestionsResponse>()
        .await?;

    info!("Fetched {} questions", resp.items.len());

    for q in resp.items {
        if let Some(deadline) = q.resolve_by {
            if Utc::now() >= deadline {
                // Check if already processed
                let existing = processed_coll
                    .find_one(doc! { "question_id": &q.id })
                    .await?;
                if existing.is_some() {
                    info!("Question {} already processed, skipping", &q.id);
                    continue;
                }

                info!("Processing question {}: {}", &q.id, &q.title);

                // Determine if prediction was completed before (or at) the deadline
                let succeeded = check_completion_logic(&q).await;

                if succeeded {
                    post_beeminder_datapoint(
                        &http,
                        &beeminder_user,
                        &beeminder_goal,
                        &beeminder_auth,
                        1.0,
                        &format!("SUCCESS: {}", &q.title),
                    )
                    .await?;
                } else {
                    // If you prefer to skip failures entirely, comment this out.
                    post_beeminder_datapoint(
                        &http,
                        &beeminder_user,
                        &beeminder_goal,
                        &beeminder_auth,
                        0.0,
                        &format!("FAILED: {}", &q.title),
                    )
                    .await?;
                }

                // Mark processed
                processed_coll
                    .insert_one(doc! {
                        "question_id": &q.id,
                        "processed_at": BsonDateTime::now()
                    })
                    .await?;
            } else {
                info!("Question {} not yet due (deadline: {})", &q.id, &deadline);
            }
        } else {
            info!("Question {} has no resolveBy field, skipping", &q.id);
        }
    }

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
        info!("Posted datapoint value={} comment={}", value, comment);
    } else {
        let body = resp.text().await.unwrap_or_else(|_| "<no body>".into());
        error!("Error posting to Beeminder: {} => {}", status, body);
    }

    Ok(())
}
