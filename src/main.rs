#![warn(unused_crate_dependencies)]
use axum::Router;
use poise::serenity_prelude::{self as serenity, CreateInteractionResponse, CreateInteractionResponseMessage};

use reqwest::Url;
use tokio::net::TcpListener;
use tokio::sync::RwLock;

use std::env;
use std::{fs, net::SocketAddr, sync::Arc, result::Result};

use commands::{report::report, db::db, loadout::loadout, edit_loadout::edit_loadout};
use issue::{Issues, NoIssueFound};
use loadout_data::{CalamityClass, LoadoutData, Stage};
use poise::{
    samples::register_globally,
    FrameworkOptions,
    FrameworkContext,
};
use serenity::{
    ActivityData,
    OnlineStatus,
    GuildId,
    ChannelId,
    GuildChannel,
    Interaction,
    GatewayIntents,
    ComponentInteractionDataKind,
    Client,
    FullEvent,
};

use shuttle_runtime::{CustomError, Service};
use shuttle_runtime::SecretStore;

use sqlx::{PgPool, Executor};
use tracing::info;

use crate::{commands::{ping::ping, help::help, playthrough::playthrough, wiki::wiki}, playthrough_data::PlaythroughData};

mod web;
mod loadout_data;
mod playthrough_data;
mod commands;
mod issue;

#[macro_export]
macro_rules! str {
    ($s: expr) => {
        $s.to_string()
    };
}

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type PoiseResult = Result<(), Error>;
pub type Context<'a> = poise::Context<'a, Data, Error>;

pub struct Data {
    pool: PgPool,
    issue_channel: GuildChannel,

    loadouts: Arc<RwLock<LoadoutData>>,
    playthroughs: Arc<RwLock<PlaythroughData>>,
    issues: Arc<RwLock<Issues>>,
}

struct PoiseAxumService {
    pub poise: Client,
    pub axum: Router,
}

#[shuttle_runtime::async_trait]
impl Service for PoiseAxumService {
    async fn bind(mut self, addr: SocketAddr) -> Result<(), shuttle_runtime::Error> {
        let web_server = axum::serve(TcpListener::bind(addr).await.unwrap(), self.axum);

        tokio::select! {
            _ = self.poise.start() => {},
            _ = web_server => {},
        }

        Ok(())
    }
}

#[shuttle_runtime::main]
async fn poise(
    #[shuttle_runtime::Secrets] secret_store: SecretStore,
    #[shuttle_shared_db::Postgres(
        local_uri = "postgresql://DatAsianBoi123:{secrets.NEON_PASS}@ep-rough-star-70439200.us-east-2.aws.neon.tech/neondb?sslmode=require"
    )] pool: PgPool,
) -> Result<PoiseAxumService, shuttle_runtime::Error> {
    env::set_var("URL", secret_store.get("URL").expect("URL not found"));

    let token = secret_store.get("TOKEN").expect("TOKEN not found");

    let schema = fs::read_to_string("static/schema.sql").expect("file exists");
    pool.execute(&schema[..]).await.map_err(CustomError::new)?;

    let loadouts = Arc::new(RwLock::new(LoadoutData::default()));
    let playthroughs = Arc::new(RwLock::new(PlaythroughData::default()));
    let issues = Arc::new(RwLock::new(Issues::default()));

    let loadouts_setup = loadouts.clone();
    let playthroughs_setup = playthroughs.clone();

    let framework = poise::Framework::builder()
        .options(FrameworkOptions {
            commands: vec![
                ping(),
                loadout(),
                edit_loadout(),
                help(),
                playthrough(),
                report(),
                db(),
                wiki(),
            ],
            event_handler: |ctx, event, framework, data| {
                Box::pin(event_handler(ctx, event, framework, data))
            },
            ..Default::default()
        })
        .setup(|ctx, ready, framework| {
            Box::pin(async move {
                register_globally(ctx, &framework.options().commands).await?;
                ctx.set_presence(Some(ActivityData::playing("TModLoader")), OnlineStatus::Online);

                *loadouts_setup.write().await = LoadoutData::load(&pool).await;
                *playthroughs_setup.write().await = PlaythroughData::load(&pool).await;
                *issues.write().await = Issues::load(&ctx.http, &pool).await;

                let guild_id: u64 = secret_store.get("ISSUE_GUILD").and_then(|id| id.parse().ok()).expect("issue guild should be valid and exists");
                let guild_id = GuildId::from(guild_id);

                let channel_id: u64 = secret_store.get("ISSUE_CHANNEL").and_then(|id| id.parse().ok()).expect("issue channel should be valid and exists");
                let channel_id = ChannelId::from(channel_id);

                let channels = guild_id.channels(&ctx.http).await?;
                let issue_channel = channels.get(&channel_id).expect("channel exists");

                let all_guilds = ctx.cache.guild_count();
                info!("loaded {} playthroughs", playthroughs_setup.read().await.active_playthroughs.len());
                info!("loaded {} issues", issues.read().await.issues.len());
                info!("helping playthroughs in {} guilds", all_guilds);
                info!("ready! logged in as {}", ready.user.tag());
                Ok(Data {
                    pool,
                    issue_channel: issue_channel.clone(),

                    loadouts: loadouts_setup,
                    playthroughs: playthroughs_setup,
                    issues,
                })
            })
        })
        .build();

    let client = Client::builder(token, GatewayIntents::GUILDS)
        .framework(framework)
        .await.expect("create client");

    Ok(PoiseAxumService { poise: client, axum: web::app(loadouts, playthroughs) })
}

async fn event_handler(ctx: &serenity::Context, event: &FullEvent, _framework: FrameworkContext<'_, Data, Error>, data: &Data) -> PoiseResult {
    match event {
        FullEvent::InteractionCreate { interaction: Interaction::Component(interaction) }
            if matches!(interaction.data.kind, ComponentInteractionDataKind::Button) && interaction.data.custom_id.starts_with("r-") => {
                if let Some(member) = &interaction.member {
                    let id = {
                        let guild = interaction.guild_id.expect("has guild id").to_guild_cached(ctx).expect("guild exists in cache");
                        let channel = guild.channels.get(&interaction.channel_id).expect("channel exists");
                        let permissions = guild.user_permissions_in(channel, member);
                        if !permissions.administrator() { return Ok(()); }
                        interaction.data.custom_id[2..].parse::<i32>().expect("issue id is a number")
                    };

                    let mut issues = data.issues.write().await;
                    let issue = issues.resolve(id, &data.pool).await.map_err(|NoIssueFound(id)| format!("issue not found: {id}"))?;

                    interaction.create_response(ctx, CreateInteractionResponse::UpdateMessage(CreateInteractionResponseMessage::new()
                            .embed(issue.create_resolved_embed())
                            .components(Vec::with_capacity(0))
                    )).await?;
                }
        }
        _ => {},
    }

    Ok(())
}

pub fn url() -> Url {
    env::var("URL").expect("URL env variable").parse().expect("URL is valid")
}

pub fn get_asset(path: &str) -> Url {
    url().join("assets/").expect("path is valid").join(path).expect("path is valid")
}

pub fn get_loadout_url(class: CalamityClass, stage: Stage) -> Url {
    let mut url = url().join("loadout/").expect("path is valid");
    url.query_pairs_mut()
        .append_pair("class", &class.to_string())
        .append_pair("stage", &format!("{stage:?}"));
    url
}

pub fn ordered<S, I>(iter: I) -> String
where
    S: ToString,
    I: IntoIterator<Item = S>,
{
    iter.into_iter()
        .map(|e| str!("1. ") + &e.to_string())
        .fold(String::new(), |prev, curr| prev + "\n" + &curr)
}

pub fn bulleted<S, I>(iter: I) -> String
where
    S: ToString,
    I: IntoIterator<Item = S>,
{
    iter.into_iter()
        .map(|e| str!("- ") + &e.to_string())
        .fold(String::new(), |prev, curr| prev + "\n" + &curr)
}

