use std::collections::HashMap;

use crate::empty_state;
use crate::types::*;
use discord_api::BotId;
use discord_api::DiscordApiRequest;
use discord_api::HttpApiCall;
use discord_api::InteractionCallbackData;
use discord_api::InteractionsCall;
use discord_api::MessagesCall;
use kinode_process_lib::{
    await_message, call_init, get_typed_state, println, set_state, Address, Message, ProcessId,
    Request, SendError,
};

pub fn send_message_to_discord(
    msg: String,
    our: &Address,
    bot: &BotId,
    discord_api_id: &ProcessId,
    interaction_id: String,
    interaction_token: Option<String>,
) -> anyhow::Result<()> {
    println!("jeeves: attempting to send message to discord: {}", msg);
    // if message is longer than 900 chars, send multiple calls
    let chunks = if msg.len() > 900 {
        let mut chunks = vec![];
        for chunk in msg.as_bytes().chunks(900) {
            chunks.push(String::from_utf8_lossy(chunk).to_string());
        }
        chunks
    } else {
        vec![msg]
    };
    let calls = if let Some(interaction_token) = interaction_token {
        println!("jeeves: interaction token found");
        chunks
            .iter()
            .map(|chunk| {
                HttpApiCall::Interactions(InteractionsCall::CreateInteractionResponse {
                    interaction_id: interaction_id.clone(),
                    interaction_token: interaction_token.clone(),
                    interaction_type: 4, // ChannelMessageWithSource
                    data: Some(InteractionCallbackData {
                        tts: None,
                        content: Some(chunk.to_string()),
                        embeds: None,
                        allowed_mentions: None,
                        flags: None,
                        components: None,
                        attachments: None,
                    }),
                })
            })
            .collect::<Vec<HttpApiCall>>()
    } else {
        println!("jeeves: interaction token not found; sending chat...");
        chunks
            .iter()
            .map(|chunk| {
                HttpApiCall::Messages(MessagesCall::Create {
                    channel_id: interaction_id.clone(),
                    content: chunk.to_string(),
                })
            })
            .collect::<Vec<HttpApiCall>>()
    };

    // Send the response to the Discord API
    for call in calls {
        Request::new()
            .target((our.node.as_ref(), discord_api_id.clone()))
            .body(serde_json::to_vec(&DiscordApiRequest::Http {
                bot: bot.clone(),
                call,
            })?)
            .expects_response(5)
            .send()?;
    }
    Ok(())
}
