use poise::{command, serenity_prelude::CreateMessage};

use crate::{Context, PoiseResult, loadout_data::{CalamityClass, Stage}};

#[command(slash_command, description_localized("en-US", "Reports a problem with a loadout"), ephemeral)]
pub async fn report(
    ctx: Context<'_>,
    #[description = "The class that the issue is in"] class: CalamityClass,
    #[description = "The stage that the issue is in"] stage: Stage,
    #[description = "The incorrect phrase"] incorrect: String,
    #[description = "The phrase that should replace the incorrect one"] correct: String,
) -> PoiseResult {
    let mut issues = ctx.data().issues.write().await;
    let issue = issues.create(ctx.author(), class, stage, incorrect, correct, &ctx.data().pool).await;

    ctx.data().issue_channel.send_message(ctx, CreateMessage::new()
        .embed(issue.create_embed())
        .components(issue.create_components()))
        .await?;

    ctx.say("Successfully reported your issue!").await?;

    Ok(())
}

