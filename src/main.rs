use anyhow::Result;
use chrono::{DateTime, Utc};
use log::{error, info};
use mongodb::{bson::doc, Client as MongoClient, Collection};
use reqwest::Client;
use serde::Deserialize;
use std::env;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
struct Question {
    id: String,
    title: String,
    resolveBy: Option<DateTime<Utc>>,
    resolved: bool,
    // you may add other fields you care about
}

#[derive(Debug, Deserialize)]
struct GetQuestionsResponse {
    items: Vec<Question>,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

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
    let processed_coll: Collection<mongodb::bson::Document> = db.collection(&processed_coll_name);

    let http = Client::new();

    info!("Fetching questions ready to resolve from Fatebook");
    let resp = http
        .get("https://fatebook.io/api/v0/getQuestions")
        .query(&[
            ("apiKey", &fatebook_api_key),
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
        if let Some(deadline) = q.resolveBy {
            if Utc::now() >= deadline {
                // Check if already processed
                let existing = processed_coll
                    .find_one(doc! { "question_id": &q.id }, None)
                    .await?;
                if existing.is_some() {
                    info!("Question {} already processed, skipping", &q.id);
                    continue;
                }

                info!("Processing question {}: {}", &q.id, &q.title);

                // Your logic to determine if prediction was completed
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
                    .insert_one(
                        doc! {
                            "question_id": &q.id,
                            "processed_at": Utc::now()
                        },
                        None,
                    )
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

async fn check_completion_logic(_q: &Question) -> bool {
    // TODO: plug in your logic to determine completion of the prediction.
    // e.g., query your task tracker, check a flag, etc.
    false
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

    if resp.status().is_success() {
        info!("Posted datapoint value={} comment={}", value, comment);
    } else {
        let body = resp.text().await?;
        error!("Error posting to Beeminder: {} => {}", resp.status(), body);
    }

    Ok(())
}
