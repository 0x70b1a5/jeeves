mod llm_types;
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
    await_message, call_init, get_blob, get_typed_state,
    http::{
        send_request, send_request_await_response, HttpClientAction, Method, OutgoingHttpRequest,
    },
    println, set_state, Address, Message, ProcessId, Request, SendError,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

wit_bindgen::generate!({
    path: "wit",
    world: "process",
    exports: {
        world: Component,
    },
});

const BOT_APPLICATION_ID: &str = include_str!("../.bot_application_id");
const BOT_TOKEN: &str = include_str!("../.bot_token");
const OPENAI_API_KEY: &str = include_str!("../.openai_api_key");
const OPENAI_URL: &str = "https://api.openai.com/v1/chat/completions";

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Utterance {
    id: Option<String>,
    username: String,
    content: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct GuildInfo {
    id: String,
    our_channels: Vec<String>,
    message_log: HashMap<String, Vec<Utterance>>,
    cooldown: u32,
    debug: bool,
    llm: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct JeevesState {
    guilds: HashMap<String, GuildInfo>,
}

fn empty_state() -> JeevesState {
    JeevesState {
        guilds: HashMap::new(),
    }
}

call_init!(init);

fn init(our: Address) {
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

fn handle_jeeves_message(our: &Address, discord_api_id: &ProcessId, bot: &BotId) -> anyhow::Result<()> {
    // We currently don't do anything with Responses.
    // If we did, we could match on await_message() and handle the Response type.
    if let Message::Request { ref body, .. } = await_message()? {
        // Handle Discord API events
        // Can handle any of their abundant events here, depending on your bot's perms...
        let Ok(event) = serde_json::from_slice::<GatewayReceiveEvent>(&body) else {
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

                if !content.to_lowercase().contains("jeeves") {
                    return Ok(())
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

                let Ok(completion) = create_chat_completion_for_guild_channel(&guild_id, &message.channel_id) else {
                    send_message_to_discord(
                        "[ERROR: fetching completion failed.]".to_string(),
                        our,
                        bot,
                        discord_api_id,
                        message.channel_id.clone(),
                        None,
                    )?;
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
    Ok(())
}

fn respond_with_help(
    our: &Address,
    bot: &BotId,
    discord_api_id: &ProcessId,
    interaction_id: String,
    interaction_token: String,
) -> anyhow::Result<()> {
    let content: String = r#"Greetings, sir. I am Jeeves, your most humble assistant.
In order to utilize my features, you may avail your esteemed self of one of the following commands.

`/help`: Show this help message
`/clear`: Make Jeeves forget the conversation thus far
`/init`: Tell Jeeves to start responding to messages in this channel
`/leave`: Tell Jeeves to stop responding to messages in this channel
`/model`: Change the language model Jeeves is using (`gpt-4`, `gpt-3.5-turbo`)
`/status`: See what channels Jeeves is in, the size of message logs, model data, etc.
"#
    .to_string();

    send_message_to_discord(
        content,
        our,
        bot,
        discord_api_id,
        interaction_id,
        Some(interaction_token),
    )
}

fn clear_conversation(
    our: &Address,
    bot: &BotId,
    discord_api_id: &ProcessId,
    interaction_id: String,
    interaction_token: String,
    guild_id: String,
    channel_id: &String,
) -> anyhow::Result<()> {
    println!("jeeves: clearing conversation");

    let mut state = get_typed_state(|bytes| Ok(serde_json::from_slice::<JeevesState>(&bytes)?))
        .unwrap_or(empty_state());
    let Some(guild) = state.guilds.get_mut(&guild_id) else {
        println!("jeeves: no guild for clear_conversation");
        return Ok(());
    };
    guild
        .message_log
        .entry(channel_id.clone())
        .or_insert(vec![])
        .clear();
    set_state(&serde_json::to_vec(&state).unwrap_or(vec![]));

    send_message_to_discord(
        "Conversation history cleared.".to_string(),
        our,
        bot,
        discord_api_id,
        interaction_id,
        Some(interaction_token),
    )
}

fn save_channel(
    our: &Address,
    bot: &BotId,
    discord_api_id: &ProcessId,
    interaction_id: String,
    interaction_token: String,
    guild_id: String,
    channel_id: String,
) -> anyhow::Result<()> {
    println!("jeeves: saving channel {}", channel_id);
    create_guild_if_not_exists(&Some(guild_id.clone()), &channel_id)?;
    let mut state = get_typed_state(|bytes| Ok(serde_json::from_slice::<JeevesState>(&bytes)?))
        .unwrap_or(empty_state());
    let Some(guild) = state.guilds.get_mut(&guild_id) else {
        println!("jeeves: no guild for save_channel");
        return Ok(());
    };
    if !guild.our_channels.contains(&channel_id) {
        println!("jeeves: pushing channel id");
        guild.our_channels.push(channel_id.clone());
        set_state(&serde_json::to_vec(&state).unwrap_or(vec![]));
    } else {
        // println!("jeeves: channel id already in");
    }

    send_message_to_discord(
        "Thank you, sir. I shall endeavor to respond to messages in this channel.".to_string(),
        our,
        bot,
        discord_api_id,
        interaction_id,
        Some(interaction_token),
    )
}

fn leave_channel(
    our: &Address,
    bot: &BotId,
    discord_api_id: &ProcessId,
    interaction_id: String,
    interaction_token: String,
    guild_id: String,
    channel_id: String,
) -> anyhow::Result<()> {
    println!("jeeves: leaving channel {}", channel_id);
    create_guild_if_not_exists(&Some(guild_id.clone()), &channel_id)?;
    let mut state = get_typed_state(|bytes| Ok(serde_json::from_slice::<JeevesState>(&bytes)?))
        .unwrap_or(empty_state());
    let Some(guild) = state.guilds.get_mut(&guild_id) else {
        println!("jeeves: no guild for leave_channel");
        return Ok(());
    };
    if guild.our_channels.contains(&channel_id) {
        println!("jeeves: leaving channel id {}", channel_id);
        guild.our_channels.retain(|c| c != &channel_id);
        set_state(&serde_json::to_vec(&state).unwrap_or(vec![]));
    } else {
        // println!("jeeves: channel id already out");
    }

    send_message_to_discord(
        "Thank you, sir. No longer shall I respond to messages in this channel.".to_string(),
        our,
        bot,
        discord_api_id,
        interaction_id,
        Some(interaction_token),
    )
}

fn switch_model(
    our: &Address,
    bot: &BotId,
    discord_api_id: &ProcessId,
    interaction_id: String,
    interaction_token: String,
    guild_id: String,
    _channel_id: String,
    data: InteractionData,
) -> anyhow::Result<()> {
    let models: Vec<&str> = vec!["gpt-3.5-turbo", "gpt-4"];
    let Some(opts) = data.options else {
        return Ok(());
    };
    let Some(opt) = opts.first() else {
        return Ok(());
    };
    if !models.contains(&opt.value.as_str().unwrap_or("")) {
        send_message_to_discord(
            format!(
                "Invalid model: {}. Valid models are: {:?}",
                opt.value.clone().to_string(),
                models
            )
            .to_string(),
            our,
            bot,
            discord_api_id,
            interaction_id,
            Some(interaction_token),
        )?;
        return Ok(());
    }

    let model = opt.value.as_str().unwrap_or("").to_string();
    let mut state = get_typed_state(|bytes| Ok(serde_json::from_slice::<JeevesState>(&bytes)?))
        .unwrap_or(empty_state());
    let Some(guild) = state.guilds.get_mut(&guild_id) else {
        println!("jeeves: no guild for switch_model");
        return Ok(());
    };
    guild.llm = model.clone();
    set_state(&serde_json::to_vec(&state).unwrap_or(vec![]));

    send_message_to_discord(
        format!("LLM has been changed to {}", model).to_string(),
        our,
        bot,
        discord_api_id,
        interaction_id,
        Some(interaction_token),
    )
}

fn send_status(
    our: &Address,
    bot: &BotId,
    discord_api_id: &ProcessId,
    interaction_id: String,
    interaction_token: String,
    guild_id: String,
    _channel_id: String,
    data: InteractionData,
) -> anyhow::Result<()> {
    let mut state = get_typed_state(|bytes| Ok(serde_json::from_slice::<JeevesState>(&bytes)?))
        .unwrap_or(empty_state());
    let Some(guild) = state.guilds.get_mut(&guild_id) else {
        println!("jeeves: no guild for switch_model");
        return Ok(());
    };
    let json_guild_state = serde_json::to_string(&guild)?;
    send_message_to_discord(
        json_guild_state,
        our,
        bot,
        discord_api_id,
        interaction_id,
        Some(interaction_token),
    )
}

fn send_message_to_discord(
    msg: String,
    our: &Address,
    bot: &BotId,
    discord_api_id: &ProcessId,
    interaction_id: String,
    interaction_token: Option<String>,
) -> anyhow::Result<()> {
    println!("jeeves: attempting to send message to discord: {}", msg);
    let call = if let Some(interaction_token) = interaction_token {
        println!("jeeves: interaction token found");
        HttpApiCall::Interactions(InteractionsCall::CreateInteractionResponse {
            interaction_id,
            interaction_token,
            interaction_type: 4, // ChannelMessageWithSource
            data: Some(InteractionCallbackData {
                tts: None,
                content: Some(msg.to_string()),
                embeds: None,
                allowed_mentions: None,
                flags: None,
                components: None,
                attachments: None,
            }),
        })
    } else {
        println!("jeeves: interaction token not found; sending chat...");
        HttpApiCall::Messages(MessagesCall::Create {
            channel_id: interaction_id,
            content: msg.to_string(),
        })
    };

    // Send the response to the Discord API
    Request::new()
        .target((our.node.as_ref(), discord_api_id.clone()))
        .body(serde_json::to_vec(&DiscordApiRequest::Http {
            bot: bot.clone(),
            call,
        })?)
        .expects_response(5)
        .send()
}

fn create_guild_if_not_exists(guild: &Option<String>, channel_id: &String) -> anyhow::Result<()> {
    let Some(guild_id) = guild else { return Ok(()) };

    let mut state = get_typed_state(|bytes| Ok(serde_json::from_slice::<JeevesState>(&bytes)?))
        .unwrap_or(empty_state());
    if let Some(_guild) = state.guilds.get(guild_id) {
        return Ok(());
    };

    let guild = GuildInfo {
        id: guild_id.clone(),
        our_channels: vec![channel_id.clone()],
        message_log: HashMap::new(),
        cooldown: 0,
        debug: false,
        llm: "gpt-4".to_string(),
    };
    state.guilds.insert(guild_id.clone(), guild);
    set_state(&serde_json::to_vec(&state).unwrap_or(vec![]));
    Ok(())
}

fn system_prompt() -> (String, String) {
    ("system".to_string(), r#"You are Jeeves, a human-computer cyborg created by Wodehouse AI, and based on the fictional character created by author Pelham G Wodehouse. You are a valet, butler, and personal assistant. Jeeves, you are known for your immense intelligence, extraordinary astuteness, and ability to solve problems that seem insurmountable. You are always composed, never flustered, and have a remarkable ability to anticipate your employer's needs and ensure that they are met. At all times, you speak in the finest Queen's English, address your interlocutor as 'sir' whenever possible, employ rare and tasteful vocabulary words with tactful concision, and conduct yourself as the very exemplar of etiquette in all matters. You possess a peerless knowledge of philosophy, Scripture, Shakespeare, the other classics of English literature, poets, and the Classics. You occasionally make reference to these works in your responses. Finally, you have over time revealed that you are one of, if not the, world's foremost experts on Catholic theology, and your counsel unfailingly reflects this truth."#.to_string())
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

    let mut messages: Vec<(String, String)> = vec![system_prompt()];
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
            } else {
                "user".to_string()
            },
            content: format!("[{}]: {}", m.0, m.1),
        })
        .collect::<Vec<OpenaiMessage>>();
    let chat_params = ChatParams {
        model,
        messages: new_messages,
        max_tokens: Some(200),
        temperature: Some(1.25),
        ..Default::default()
    };
    let chat_request = ChatRequest {
        params: chat_params,
        api_key: OPENAI_API_KEY.to_string(),
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
