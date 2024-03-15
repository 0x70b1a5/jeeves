mod llm_types;
use kinode::process::standard::print_to_terminal;
use kinode_process_lib::get_blob;
use kinode_process_lib::http::bind_ws_path;
use kinode_process_lib::http::send_response;
use kinode_process_lib::http::serve_ui;
use kinode_process_lib::http::HttpServerRequest;
use kinode_process_lib::http::StatusCode;
use llm_types::openai::ChatParams;
use llm_types::openai::ChatRequest;
use llm_types::openai::LLMRequest;
use llm_types::openai::LLMResponse;
use llm_types::openai::Message as OpenaiMessage;

use discord_api::{
    ApplicationCommandOption, ApplicationCommandOptionType, ApplicationCommandType, BotId,
    CommandsCall, DiscordApiRequest, GatewayReceiveEvent, HttpApiCall, InteractionCallbackData,
    InteractionData, InteractionsCall, MessagesCall, NewApplicationCommand,
};
use kinode_process_lib::{
    await_message, call_init, get_typed_state, println, set_state, Address, Message, ProcessId,
    Request, SendError,
};
use std::collections::HashMap;

mod commands;
mod consts;
mod discord;
mod types;
use crate::commands::*;
use crate::consts::*;
use crate::discord::*;
use crate::types::*;

wit_bindgen::generate!({
    path: "wit",
    world: "process",
    exports: {
        world: Component,
    },
});

call_init!(init);

fn init(our: Address) {
    // Bind UI files to routes; index.html is bound to "/"
    serve_ui(&our, "ui", true, true, vec!["/"]).unwrap();

    // // Bind HTTP path /messages
    // bind_http_path("/messages", true, false).unwrap();

    // Bind WebSocket path
    bind_ws_path("/", true, true).unwrap();

    let intents = 8704; // 512 Read + 8192 Manage Messages
    let bot = BotId::new(BOT_TOKEN.trim().to_string(), intents);

    // Spawn the API process
    let result = match init_discord_api(&our, &bot) {
        Ok(result) => result,
        Err(e) => {
            println!("jeeves: error initiating bot: {e:?}");
            panic!();
        }
    };

    if let Err(e) = result {
        println!("jeeves: error initiating bot: {e:?}");
        panic!();
    }

    println!("{our}: discord_api spawned");

    // add ourselves to the homepage
    Request::to(("our", "homepage", "homepage", "sys"))
        .body(
            serde_json::json!({
                "Add": {
                    "label": "Jeeves",
                    "icon": ICON,
                    "path": "/", // just our root
                }
            })
            .to_string()
            .as_bytes()
            .to_vec(),
        )
        .send()
        .unwrap();

    // Register all the commands the bot will handle
    let help_command = HttpApiCall::Commands(CommandsCall::CreateApplicationCommand {
        application_id: BOT_APPLICATION_ID.trim().to_string(),
        command: NewApplicationCommand {
            name: "help".to_string(),
            description: Some("Show help".to_string()),
            command_type: Some(ApplicationCommandType::ChatInput.as_u8()),
            options: None,
        },
    });

    let clear_command = HttpApiCall::Commands(CommandsCall::CreateApplicationCommand {
        application_id: BOT_APPLICATION_ID.trim().to_string(),
        command: NewApplicationCommand {
            name: "clear".to_string(),
            description: Some("Make Jeeves forget the conversation thus far".to_string()),
            command_type: Some(ApplicationCommandType::ChatInput.as_u8()),
            options: None,
        },
    });

    let init_command = HttpApiCall::Commands(CommandsCall::CreateApplicationCommand {
        application_id: BOT_APPLICATION_ID.trim().to_string(),
        command: NewApplicationCommand {
            name: "init".to_string(),
            description: Some("Tell Jeeves to respond to posts in this channel".to_string()),
            command_type: Some(ApplicationCommandType::ChatInput.as_u8()),
            options: None,
        },
    });

    let leave_command = HttpApiCall::Commands(CommandsCall::CreateApplicationCommand {
        application_id: BOT_APPLICATION_ID.trim().to_string(),
        command: NewApplicationCommand {
            name: "leave".to_string(),
            description: Some("Tell Jeeves to leave this channel".to_string()),
            command_type: Some(ApplicationCommandType::ChatInput.as_u8()),
            options: None,
        },
    });

    let status_command = HttpApiCall::Commands(CommandsCall::CreateApplicationCommand {
        application_id: BOT_APPLICATION_ID.trim().to_string(),
        command: NewApplicationCommand {
            name: "status".to_string(),
            description: Some(
                "See what channels Jeeves is in, the size of message logs, model data, etc."
                    .to_string(),
            ),
            command_type: Some(ApplicationCommandType::ChatInput.as_u8()),
            options: None,
        },
    });

    let model_command = HttpApiCall::Commands(CommandsCall::CreateApplicationCommand {
        application_id: BOT_APPLICATION_ID.trim().to_string(),
        command: NewApplicationCommand {
            name: "model".to_string(),
            description: Some("Change the LLM that Jeeves will use".to_string()),
            command_type: Some(ApplicationCommandType::ChatInput.as_u8()),
            options: Some(vec![ApplicationCommandOption {
                name: "model".to_string(),
                name_localizations: None,
                description_localizations: None,
                description: "The model to use".to_string(),
                option_type: ApplicationCommandOptionType::String.as_u8(),
                required: Some(true),
            }]),
        },
    });

    let commands = vec![
        help_command,
        clear_command,
        init_command,
        leave_command,
        status_command,
        model_command,
    ];

    let discord_api_id = ProcessId::new(Some("discord_api_runner"), our.package(), our.publisher());

    for command in commands {
        Request::new()
            .target((our.node.as_ref(), discord_api_id.clone()))
            .body(
                serde_json::to_vec(&DiscordApiRequest::Http {
                    bot: bot.clone(),
                    call: command,
                })
                .unwrap(),
            )
            .expects_response(5)
            .send()
            .expect("jeeves: failed to register command");
    }

    let state = get_typed_state(|bytes| Ok(serde_json::from_slice::<JeevesState>(&bytes)?))
        .unwrap_or(empty_state());
    set_state(&serde_json::to_vec(&state).unwrap_or(vec![]));

    loop {
        match handle_jeeves_message(&our, &discord_api_id, &bot) {
            Ok(()) => {}
            Err(e) => {
                println!("jeeves: error: {e:?}");
            }
        };
    }
}

fn handle_jeeves_message(
    our: &Address,
    discord_api_id: &ProcessId,
    bot: &BotId,
) -> anyhow::Result<()> {
    match await_message() {
        Ok(Message::Request { ref body, .. }) => {
            // Handle Discord API events
            // Can handle any of their abundant events here, depending on your bot's perms...
            let Ok(event) = serde_json::from_slice::<GatewayReceiveEvent>(&body) else {
                print_to_terminal(
                    0,
                    format!("discord event: {:?}", String::from_utf8_lossy(body)).as_str(),
                );
                return Ok(());
            };

            match event {
                GatewayReceiveEvent::InteractionCreate(interaction) => {
                    let Some(data) = interaction.data else {
                        println!("jeeves: got interaction without data: {:?}", interaction);
                        return Ok(());
                    };
                    let Some(channel_id) = interaction.channel_id else {
                        return Ok(());
                    };
                    let Some(guild_id) = interaction.guild_id else {
                        println!("jeeves: no guild id for interaction_create");
                        return Ok(());
                    };
                    // create_guild_if_not_exists(&interaction.guild_id, &channel_id)?;
                    match data.name.as_str() {
                        "help" => {
                            let _ = respond_with_help(
                                &our,
                                &bot,
                                &discord_api_id,
                                interaction.id,
                                interaction.token,
                            );
                        }
                        "clear" => {
                            let _ = clear_conversation(
                                &our,
                                &bot,
                                &discord_api_id,
                                interaction.id,
                                interaction.token,
                                guild_id,
                                &channel_id,
                            );
                        }
                        "init" => {
                            let _ = save_channel(
                                &our,
                                &bot,
                                &discord_api_id,
                                interaction.id,
                                interaction.token,
                                guild_id,
                                channel_id,
                            );
                        }
                        "leave" => {
                            let _ = leave_channel(
                                &our,
                                &bot,
                                &discord_api_id,
                                interaction.id,
                                interaction.token,
                                guild_id,
                                channel_id,
                            );
                        }
                        "model" => {
                            let _ = switch_model(
                                &our,
                                &bot,
                                &discord_api_id,
                                interaction.id,
                                interaction.token,
                                guild_id,
                                channel_id,
                                data,
                            )?;
                        }
                        "status" => {
                            let _ = send_status(
                                &our,
                                &bot,
                                &discord_api_id,
                                interaction.id,
                                interaction.token,
                                guild_id,
                                channel_id,
                                data,
                            )?;
                        }
                        _ => {}
                    }
                }
                GatewayReceiveEvent::MessageCreate(message) => {
                    let mut state =
                        get_typed_state(|bytes| Ok(serde_json::from_slice::<JeevesState>(&bytes)?))
                            .unwrap_or(empty_state());
                    let Some(guild_id) = message.guild_id else {
                        println!("jeeves: got message without guild: {:?}", message);
                        return Ok(());
                    };
                    let Some(author) = message.author else {
                        println!("jeeves: got message without author");
                        return Ok(());
                    };
                    if author.username == "Jeeves" {
                        return Ok(());
                    }
                    let Some(guild) = state.guilds.get_mut(&guild_id) else {
                        // println!("jeeves: message from outside guild: {}", guild_id);
                        return Ok(());
                    };
                    if !guild.our_channels.contains(&message.channel_id) {
                        // println!("jeeves: got message in channel not in our channels: {}", message.channel_id);
                        return Ok(());
                    }
                    if guild.cooldown > 0 {
                        println!("jeeves: guild is on cooldown: {}", guild.id);
                        return Ok(());
                    }
                    let Some(content) = message.content else {
                        println!("jeeves: got message without content");
                        return Ok(());
                    };

                    let mut should_respond = false;
                    if content.to_lowercase().contains("jeeves") {
                        should_respond = true;
                    } else if let Some(mentions) = message.mentions {
                        if mentions.iter().any(|u| u.username == "Jeeves") {
                            should_respond = true;
                        }
                    };

                    if !should_respond {
                        return Ok(());
                    }

                    // we get dupe message events sometimes
                    if guild
                        .message_log
                        .get(&message.channel_id)
                        .unwrap_or(&vec![])
                        .iter()
                        .any(|m| m.id == Some(message.id.clone()))
                    {
                        return Ok(());
                    }

                    guild
                        .message_log
                        .entry(message.channel_id.clone())
                        .or_insert(vec![])
                        .push(Utterance {
                            id: Some(message.id.clone()),
                            username: author.username,
                            content,
                        });
                    set_state(&serde_json::to_vec(&state).unwrap_or(vec![]));

                    let completion =
                        create_chat_completion_for_guild_channel(&guild_id, &message.channel_id);
                    if let Err(e) = completion {
                        send_message_to_discord(
                            format!("[ERROR: fetching completion failed: {}]", e).to_string(),
                            our,
                            bot,
                            discord_api_id,
                            message.channel_id.clone(),
                            None,
                        )?;
                        return Ok(());
                    }

                    let Ok(completion) = completion else {
                        return Ok(());
                    };

                    println!("jeeves: got completion: {}", completion);

                    send_message_to_discord(
                        completion.clone(),
                        our,
                        bot,
                        discord_api_id,
                        message.channel_id.clone(),
                        None,
                    )?;

                    let mut state =
                        get_typed_state(|bytes| Ok(serde_json::from_slice::<JeevesState>(&bytes)?))
                            .unwrap_or(empty_state());
                    let Some(guild) = state.guilds.get_mut(&guild_id) else {
                        return Ok(());
                    };
                    guild
                        .message_log
                        .entry(message.channel_id.clone())
                        .or_insert(vec![])
                        .push(Utterance {
                            id: None,
                            username: "Jeeves".to_string(),
                            content: completion,
                        });
                    set_state(&serde_json::to_vec(&state).unwrap_or(vec![]));
                }
                _ => {}
            }
        }
        Ok(Message::Response { ref body, .. }) => {
            // Responses currently only come in from popping Timers.
            // Timers are right now used for queueing multi-stage messages.
            // Each Timer has a context of what message to send, and the data necessary to send it.

            // TODO shrimplement

            println!("jeeves: got response: {:?}", String::from_utf8_lossy(body));
        }
        _ => {}
    }
    Ok(())
}

fn create_chat_completion_for_guild_channel(
    guild_id: &String,
    channel_id: &String,
) -> anyhow::Result<String> {
    let state = get_typed_state(|bytes| Ok(serde_json::from_slice::<JeevesState>(&bytes)?))
        .unwrap_or(empty_state());
    let Some(guild) = state.guilds.get(guild_id) else {
        return Ok("".to_string());
    };
    if !guild.our_channels.contains(&channel_id) {
        return Ok("".to_string());
    }

    let mut messages: Vec<(String, String)> =
        vec![("system".to_string(), guild.system_prompt.clone())];
    for msg in guild.message_log.get(channel_id).unwrap_or(&vec![]).clone() {
        messages.push((msg.username.clone(), msg.content.clone()));
    }

    let mut model = guild.llm.clone();
    if model.len() == 0 {
        model = "gpt-3.5-turbo".to_string();
    }
    create_chat_completion(messages, model)
}

fn create_chat_completion(
    messages: Vec<(String, String)>,
    model: String,
) -> anyhow::Result<String> {
    let new_messages = messages
        .iter()
        .map(|m| OpenaiMessage {
            role: if m.0 == "Jeeves" {
                "assistant".to_string()
            } else if m.0 == "system" {
                "system".to_string()
            } else {
                "user".to_string()
            },
            content: format!("[{}]: {}", m.0, m.1),
        })
        .collect::<Vec<OpenaiMessage>>();
    let chat_params = ChatParams {
        model,
        messages: new_messages,
        max_tokens: Some(900),
        temperature: Some(1.25),
        ..Default::default()
    };
    let chat_request = ChatRequest {
        params: chat_params,
        api_key: OPENAI_API_KEY.trim().to_string(),
    };
    let request = LLMRequest::Chat(chat_request);
    let msg = Request::new()
        .target(Address::new(
            "our",
            ProcessId::new(Some("openai"), "llm", "kinode"),
        ))
        .body(request.to_bytes())
        .send_and_await_response(30)??;
    let response = LLMResponse::parse(msg.body())?;
    if let LLMResponse::Chat(chat) = response {
        let completion = chat.to_chat_response();
        let t = completion.to_string().replace("[Jeeves]:", "");
        println!("jeeves says: {}", t);
        Ok(t)
    } else {
        Err(anyhow::Error::msg("Error querying OpenAI: wrong result"))
    }
}

fn init_discord_api(
    our: &Address,
    bot_id: &BotId,
) -> Result<Result<Message, SendError>, anyhow::Error> {
    Request::new()
        .target((
            our.node.as_ref(),
            ProcessId::new(Some("discord_api_runner"), our.package(), our.publisher()),
        ))
        .body(serde_json::to_vec(&DiscordApiRequest::Connect(
            bot_id.clone(),
        ))?)
        .send_and_await_response(5)
}

fn handle_frontend_request(
    our: &Address,
    our_channel_id: &mut u32,
    source: &Address,
    body: &[u8],
) -> anyhow::Result<()> {
    /*
     * frontend does the following:
     * 1. join or leave a discord guild
     * 1. change the llm system prompt
     * 1. change the llm model, api key, and api endpoint; or, change to a local model, and configure the local endpoint
     * 1. set a backup model with the above info
     * 1. add or remove channels from the bot
     * 1. configure response conditions
     *   a. when pinged
     *   a. when a word or phrase shows up
     *   a. every message
     * 1. define roles to ignore, and roles to listen to, for both messages and bot commands
     * 1. define a list of users to ignore
     */
    Ok(())
}

fn handle_http_server_request(
    our: &Address,
    our_channel_id: &mut u32,
    source: &Address,
    body: &[u8],
) -> anyhow::Result<()> {
    let Ok(server_request) = serde_json::from_slice::<HttpServerRequest>(body) else {
        // Fail silently if we can't parse the request
        return Ok(());
    };

    match server_request {
        HttpServerRequest::WebSocketOpen { channel_id, .. } => {
            // Set our channel_id to the newly opened channel
            // Note: this code could be improved to support multiple channels
            *our_channel_id = channel_id;
        }
        HttpServerRequest::WebSocketPush { .. } => {
            let Some(blob) = get_blob() else {
                return Ok(());
            };

            handle_frontend_request(our, our_channel_id, source, &blob.bytes)?;
        }
        HttpServerRequest::WebSocketClose(_channel_id) => {}
        HttpServerRequest::Http(request) => {
            match request.method()?.as_str() {
                // Get state
                "GET" => {
                    let mut headers = HashMap::new();
                    headers.insert("Content-Type".to_string(), "application/json".to_string());
                    let state =
                        get_typed_state(|bytes| Ok(serde_json::from_slice::<JeevesState>(&bytes)?))
                            .unwrap_or(empty_state());
                    let state = serde_json::to_vec(&state)?;

                    send_response(StatusCode::OK, Some(headers), state);
                }
                _ => {
                    // Method not allowed
                    send_response(StatusCode::METHOD_NOT_ALLOWED, None, vec![]);
                }
            }
        }
    };

    Ok(())
}
