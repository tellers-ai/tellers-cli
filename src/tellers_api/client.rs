use crate::tellers_api::models::{ChatChunk, ChatCreateRequest};

#[derive(Debug)]
pub struct TellersClient {
    pub _api_base: String,
    pub _api_key: String,
}

impl TellersClient {
    pub fn new_from_env() -> Result<Self, &'static str> {
        let api_base = std::env::var("TELLERS_API_BASE")
            .unwrap_or_else(|_| "https://api.tellers.ai".to_string());
        let api_key = std::env::var("TELLERS_API_KEY").map_err(|_| "TELLERS_API_KEY not set")?;
        Ok(Self {
            _api_base: api_base,
            _api_key: api_key,
        })
    }

    pub fn create_chat(&self, prompt: &str, _full_auto: bool) -> Result<String, &'static str> {
        let _req = ChatCreateRequest {
            prompt: prompt.to_string(),
        };
        // Placeholder: call POST /chats and return the chat_id
        Ok("chat_placeholder_id".to_string())
    }

    pub fn stream_chat(&self, _chat_id: &str) -> Result<Vec<ChatChunk>, &'static str> {
        // Placeholder: this should stream chunks from the API (SSE/Websocket/HTTP chunked)
        Ok(vec![
            ChatChunk::Text("Hello from tellers.ai".into()),
            ChatChunk::Text("\nThis is a streamed response stub.".into()),
        ])
    }
}
