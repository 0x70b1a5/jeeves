use std::collections::HashMap;

use crate::discord::*;
use crate::empty_state;
use crate::types::*;
use discord_api::BotId;
use discord_api::InteractionData;
use kinode_process_lib::{
    await_message, call_init, get_typed_state, println, set_state, Address, Message, ProcessId,
    Request, SendError,
};

pub fn respond_with_help(
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

pub fn clear_conversation(
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

pub fn save_channel(
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

pub fn leave_channel(
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

pub fn switch_model(
    our: &Address,
    bot: &BotId,
    discord_api_id: &ProcessId,
    interaction_id: String,
    interaction_token: String,
    guild_id: String,
    _channel_id: String,
    data: InteractionData,
) -> anyhow::Result<()> {
    let models: Vec<&str> = vec![
        "local",
        "gpt-3.5-turbo",
        "gpt-4",
        "gpt-4-1106-preview",
        "gpt-4-turbo-preview",
    ];
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

pub fn send_status(
    our: &Address,
    bot: &BotId,
    discord_api_id: &ProcessId,
    interaction_id: String,
    interaction_token: String,
    guild_id: String,
    channel_id: String,
    data: InteractionData,
) -> anyhow::Result<()> {
    let mut state = get_typed_state(|bytes| Ok(serde_json::from_slice::<JeevesState>(&bytes)?))
        .unwrap_or(empty_state());
    let Some(guild) = state.guilds.get_mut(&guild_id) else {
        send_message_to_discord(
            "[ERROR: no state found for this guild.]".to_string(),
            our,
            bot,
            discord_api_id,
            interaction_id,
            Some(interaction_token),
        )?;
        return Ok(());
    };
    let msg = format!(
        r#"
**Guild**: {}
**Channels**: {}
**Message Count (this channel)**: {}
**Model**: {}"#,
        guild_id,
        format!("#{}", guild.our_channels.join(", #")),
        guild.message_log.get(&channel_id).unwrap_or(&vec![]).len(),
        guild.llm
    )
    .to_string();

    send_message_to_discord(
        msg,
        our,
        bot,
        discord_api_id,
        interaction_id,
        Some(interaction_token),
    )
}

pub fn create_guild_if_not_exists(
    guild: &Option<String>,
    channel_id: &String,
) -> anyhow::Result<()> {
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
        llm: "gpt-3.5-turbo".to_string(),
        system_prompt: system_prompt().1,
        response_schema: BotResponseSchema::EveryMessage,
        listen_to_roles: vec![],
        ignore_roles: vec![],
        listen_to_users: vec![],
        ignore_users: vec![],
    };
    state.guilds.insert(guild_id.clone(), guild);
    set_state(&serde_json::to_vec(&state).unwrap_or(vec![]));
    Ok(())
}
