use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatCreateRequest {
    pub prompt: String,
}

#[derive(Debug)]
pub enum ChatChunk {
    Text(String),
}
