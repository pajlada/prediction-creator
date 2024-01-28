use std::env;
use std::io::Write;

use clap::Parser;
use console::Term;
use dialoguer::theme::ColorfulTheme;
use dialoguer::Select;
use twitch_api::helix::predictions::end_prediction::EndPrediction;
use twitch_api::helix::predictions::{
    create_prediction, end_prediction, get_predictions, Prediction,
};
use twitch_api::helix::HelixClient;
use twitch_api::twitch_oauth2::{TwitchToken, UserToken};
use twitch_api::types::{PredictionIdRef, PredictionStatus, UserId};

async fn start_prediction(
    client: &HelixClient<'_, reqwest::Client>,
    token: &UserToken,
    channel_id: &UserId,
    title: &str,
    options: &[String],
    prediction_window: i64,
) -> anyhow::Result<create_prediction::CreatePredictionResponse> {
    let request = create_prediction::CreatePredictionRequest::new();
    let outcomes: Vec<create_prediction::NewPredictionOutcome> = options
        .iter()
        .map(create_prediction::NewPredictionOutcome::new)
        .collect();
    let body = create_prediction::CreatePredictionBody::new(
        channel_id,
        title,
        &outcomes,
        prediction_window,
    );

    let response = client
        .req_post(request, body, token)
        .await
        .map_err(|e| anyhow::anyhow!(e))?
        .data;

    Ok(response)
}

async fn get_last_prediction(
    client: &HelixClient<'_, reqwest::Client>,
    token: &UserToken,
    channel_id: &UserId,
) -> anyhow::Result<Option<Prediction>> {
    let mut request = get_predictions::GetPredictionsRequest::broadcaster_id(channel_id);
    request.first = Some(1);

    let mut response = client.req_get(request, token).await?.data;

    if let Some(last_prediction) = response.pop() {
        if last_prediction.status == PredictionStatus::Active
            || last_prediction.status == PredictionStatus::Locked
        {
            Ok(Some(last_prediction))
        } else {
            Ok(None)
        }
    } else {
        Ok(None)
    }
}

async fn end_prediction<'a>(
    client: &'a HelixClient<'a, reqwest::Client>,
    token: &'a UserToken,
    channel_id: &'a UserId,
    prediction_id: &'a PredictionIdRef,
    new_status: PredictionStatus,
    winning_outcome_id: Option<String>,
) -> anyhow::Result<end_prediction::EndPrediction> {
    let request = end_prediction::EndPredictionRequest::new();
    let mut body = end_prediction::EndPredictionBody::new(channel_id, prediction_id, new_status);
    if let Some(winning_outcome_id) = winning_outcome_id {
        body.winning_outcome_id = Some(std::borrow::Cow::Owned(winning_outcome_id.into()));
    }

    let response = client.req_patch(request, body, token).await?.data;

    Ok(response)
}

/// A very simple utility to search for a string across multiple files.
#[derive(Debug, Parser)]
#[clap(name = "prediction-creator")]
pub struct App {
    /// The title of the prediction
    #[clap(long)]
    title: String,

    /// Outcomes. At least 2 must be provided, at most 5 must be provided
    #[clap(long)]
    outcome: Vec<String>,

    /// Duration of the outcome in seconds
    #[clap(long, default_value = "30")]
    prediction_window: i64,
}

fn parse_args() -> anyhow::Result<App> {
    let app = App::try_parse()?;

    let num_outcomes = app.outcome.len();

    if num_outcomes < 2 {
        anyhow::bail!(
            "You must provide at least 2 outcomes with --outcome, you provided {num_outcomes}"
        )
    }

    if num_outcomes > 5 {
        anyhow::bail!(
            "You may provide at most 5 outcomes with --outcome, you provided {num_outcomes}"
        )
    }

    Ok(app)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let app = parse_args()?;

    let mut term = Term::stdout();

    // Create the HelixClient, which is used to make requests to the Twitch API
    let client: HelixClient<reqwest::Client> = HelixClient::default();
    let access_token = env::var("TWITCH_ACCESS_TOKEN")?;
    // Create a UserToken, which is used to authenticate requests
    let token = UserToken::from_token(&client, access_token.into()).await?;

    let broadcaster = token.validate_token(&client).await?;
    let broadcaster_login = broadcaster.login.expect("token to contain a login");
    let broadcaster_user_id = broadcaster.user_id.expect("token to contain a user id");

    let prediction = if let Some(current_prediction) =
        get_last_prediction(&client, &token, &broadcaster_user_id).await?
    {
        writeln!(
            term,
            "Found already active prediction: {}",
            current_prediction.title
        )?;
        current_prediction
    } else {
        writeln!(
            term,
            "Starting prediction for {} ({}): {}",
            console::style(broadcaster_login).bold(),
            broadcaster_user_id,
            app.title
        )?;

        start_prediction(
            &client,
            &token,
            &broadcaster_user_id,
            &app.title,
            &app.outcome,
            app.prediction_window,
        )
        .await?
    };
    let options = prediction.outcomes;

    let mut items: Vec<String> = options
        .iter()
        .enumerate()
        .map(|(i, outcome)| format!("[{}] {}", i + 1, outcome.title.clone()))
        .collect();

    items.push("CANCEL".to_string());

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("your selection please")
        .default(0)
        .items(&items)
        .interact()?;

    let response = if let Some(selected_outcome) = options.get(selection) {
        writeln!(term, "Resolving with this outcome {selected_outcome:?}")?;
        end_prediction(
            &client,
            &token,
            &broadcaster_user_id,
            &prediction.id,
            PredictionStatus::Resolved,
            Some(selected_outcome.id.clone()),
        )
        .await?
    } else {
        writeln!(term, "{}", console::style("Cancelling").bold())?;
        end_prediction(
            &client,
            &token,
            &broadcaster_user_id,
            &prediction.id,
            PredictionStatus::Canceled,
            None,
        )
        .await?
    };

    match response {
        EndPrediction::Success(ref _success) => {
            // TODO: Print successful outcome
            writeln!(term, "Successfully ended prediction")?;
        }
        EndPrediction::MissingQuery => {
            writeln!(term, "ERROR: Bad prediction body: {:?}", response)?;
        }
        EndPrediction::AuthFailed => {
            writeln!(term, "ERROR: Auth failed: {:?}", response)?;
        }
        unknown => unimplemented!("Unimplemented end_prediction response {unknown:?}"),
    }

    Ok(())
}
