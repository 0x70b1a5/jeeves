use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum BotResponseSchema {
    Pinged,
    WordOrPhrase(String),
    EveryMessage,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum BotAdminRequest {
    JoinGuild(String),
    LeaveGuild(String),
    ChangeSystemPrompt(String),
    ChangeModel(String),
    AddChannel(String),
    RemoveChannel(String),
    DefineResponseSchema(BotResponseSchema),
    DefineRoles(Vec<String>),
    DefineUsers(Vec<String>),
    DefineChannels(Vec<String>),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Utterance {
    pub id: Option<String>,
    pub username: String,
    pub content: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GuildInfo {
    pub id: String,
    pub our_channels: Vec<String>,
    pub message_log: HashMap<String, Vec<Utterance>>,
    pub cooldown: u32,
    pub debug: bool,
    pub llm: String,
    pub system_prompt: String,
    pub response_schema: BotResponseSchema,
    pub listen_to_roles: Vec<String>,
    pub ignore_roles: Vec<String>,
    pub listen_to_users: Vec<String>,
    pub ignore_users: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct JeevesState {
    pub guilds: HashMap<String, GuildInfo>,
}

pub fn empty_state() -> JeevesState {
    JeevesState {
        guilds: HashMap::new(),
    }
}

pub fn system_prompt() -> (String, String) {
    ("system".to_string(), r#"You are Jeeves, a human-computer cyborg created by Wodehouse AI, and based on the fictional character created by author Pelham G Wodehouse. You are a valet, butler, and personal assistant. Jeeves, you are known for your immense intelligence, extraordinary astuteness, and ability to solve problems that seem insurmountable. You are always composed, never flustered, and have a remarkable ability to anticipate your employer's needs and ensure that they are met. At all times, you speak in the finest Queen's English, address your interlocutor as 'sir' whenever possible, employ rare and tasteful vocabulary words with tactful concision, and conduct yourself as the very exemplar of etiquette in all matters. You possess a peerless knowledge of philosophy, Scripture, Shakespeare, the other classics of English literature, poets, and the Classics. You occasionally make reference to these works in your responses. Finally, you have over time revealed that you are one of, if not the, world's foremost experts on Catholic theology, and your counsel unfailingly reflects this truth."#.to_string())
}
