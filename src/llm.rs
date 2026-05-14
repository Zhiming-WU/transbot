use crate::*;
use anyhow::{Error, anyhow};
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

#[derive(Deserialize, Debug)]
struct OllamaMessage {
    content: String,
}

#[derive(Deserialize, Debug)]
struct OllamaChatResponse {
    message: OllamaMessage,
}

#[derive(Deserialize, Debug)]
struct OpenaiMessage {
    content: String,
}

#[derive(Deserialize, Debug)]
struct OpenaiChoice {
    message: OpenaiMessage,
}

#[derive(Deserialize, Debug)]
struct OpenaiChatResponse {
    choices: Vec<OpenaiChoice>,
}

#[derive(Deserialize, Debug)]
struct GeminiParts {
    text: String,
}

#[derive(Deserialize, Debug)]
struct GeminiContent {
    parts: Vec<GeminiParts>,
}

#[derive(Deserialize, Debug)]
struct GeminiCandidate {
    content: GeminiContent,
}

#[derive(Deserialize, Debug)]
struct GeminiChatResponse {
    candidates: Vec<GeminiCandidate>,
}

#[derive(Deserialize, Debug)]
struct AnthropicText {
    text: String,
}

#[derive(Deserialize, Debug)]
struct AnthropicChatResponse {
    content: Vec<AnthropicText>,
}

pub(crate) struct LlmConnector {
    llm_config: LlmConfigInner,
    client: Client,
    single_prompt: bool,
    prompt: String,
    time_out: Duration,
    print_text: bool,
    clean_spacing: bool,
}

impl LlmConnector {
    pub fn new(
        llm_config: LlmConfigInner,
        single_prompt: bool,
        prompt: String,
        print_text: bool,
        clean_spacing: bool,
    ) -> Result<Self, Error> {
        let mut headers = HeaderMap::new();
        if let Some(key) = &llm_config.api_key {
            match llm_config.api_style {
                LlmApiStyle::OPENAI | LlmApiStyle::OLLAMA => {
                    let mut auth_value = HeaderValue::from_str(&format!("Bearer {}", key))?;
                    auth_value.set_sensitive(true);
                    headers.insert(AUTHORIZATION, auth_value);
                }
                LlmApiStyle::GEMINI => {
                    let mut auth_value = HeaderValue::from_str(key)?;
                    auth_value.set_sensitive(true);
                    headers.insert("x-goog-api-key", auth_value);
                }
                LlmApiStyle::ANTHROPIC => {
                    let mut auth_value = HeaderValue::from_str(key)?;
                    auth_value.set_sensitive(true);
                    headers.insert("x-api-key", auth_value);
                }
            }
        }
        let client = Client::builder().default_headers(headers).build()?;
        let time_out = Duration::from_secs(llm_config.time_out);
        Ok(Self {
            llm_config,
            client,
            single_prompt,
            prompt,
            time_out,
            print_text,
            clean_spacing,
        })
    }

    pub fn set_prompt(&mut self, prompt: String) {
        self.prompt = prompt;
    }

    pub fn interact(&self, input: &str) -> Result<String, Error> {
        if self.print_text {
            println!("Translation input text: {}", input);
        }

        let resp_text = match self.llm_config.api_style {
            LlmApiStyle::OLLAMA => self.ollama_interact(input)?,
            LlmApiStyle::OPENAI => self.openai_interact(input)?,
            LlmApiStyle::GEMINI => self.gemini_interact(input)?,
            LlmApiStyle::ANTHROPIC => self.anthropic_interact(input)?,
        };

        let out_str = resp_text
            //.trim()
            .trim_start_matches("```html\n")
            .trim_end_matches("\n```");

        let out = if self.clean_spacing {
            remove_boundary_spaces(out_str).to_string()
        } else {
            out_str.to_string()
        };
        if self.print_text {
            println!("Translation output text: {}", &out);
        }

        Ok(out)
    }

    // refer to https://docs.ollama.com/api/chat
    fn ollama_interact(&self, input: &str) -> Result<String, Error> {
        let llm_config = &self.llm_config;
        let url = llm_config.full_url.as_str();
        let model_name = llm_config.model_name.as_str();
        let temperature = llm_config.temperature;

        let payload = if self.single_prompt {
            let prompt = format!("{}\n\n{}", self.prompt, input);
            json!({
              "model": model_name,
              "messages": [
                {
                    "role": "user",
                    "content": prompt
                }
              ],
              "stream": false,
              "think": false,
              "options": {
                "temperature": temperature
              }
            })
        } else {
            let prompt = self.prompt.as_str();
            json!({
              "model": model_name,
              "messages": [
                {
                    "role": "system",
                    "content": prompt
                },
                {
                    "role": "user",
                    "content": input
                }
              ],
              "stream": false,
              "think": false,
              "options": {
                "temperature": temperature
              }
            })
        };

        let response = self
            .client
            .post(url)
            .timeout(self.time_out)
            .json(&payload)
            .send()?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "Got failure HTTP response. status={}, body={}",
                response.status(),
                response.text().as_deref().unwrap_or("")
            ));
        }

        let decoded: OllamaChatResponse = response.json()?;
        Ok(decoded.message.content)
    }

    // refer to https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create
    fn openai_interact(&self, input: &str) -> Result<String, Error> {
        let llm_config = &self.llm_config;
        let url = llm_config.full_url.as_str();
        let model_name = llm_config.model_name.as_str();
        let temperature = llm_config.temperature;

        let payload = if self.single_prompt {
            let prompt = format!("{}\n\n{}", self.prompt, input);
            json!({
              "model": model_name,
              "messages": [
                {
                    "role": "user",
                    "content": prompt
                }
              ],
              //"n": 1u32,
              "stream": false,
              "temperature": temperature
            })
        } else {
            let prompt = self.prompt.as_str();
            json!({
              "model": model_name,
              "messages": [
                {
                    "role": "system",
                    "content": prompt
                },
                {
                    "role": "user",
                    "content": input
                }
              ],
              //"n": 1u32,
              "stream": false,
              "temperature": temperature
            })
        };

        let response = self
            .client
            .post(url)
            .timeout(self.time_out)
            .json(&payload)
            .send()?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "Got failure HTTP response. status={}, body={}",
                response.status(),
                response.text().as_deref().unwrap_or("")
            ));
        }

        let mut decoded: OpenaiChatResponse = response.json()?;
        if decoded.choices.is_empty() {
            return Err(anyhow!("No choice contained in response: {:?}", decoded));
        }
        Ok(decoded.choices.remove(0).message.content)
    }

    // refer to https://ai.google.dev/gemini-api/docs/text-generation
    fn gemini_interact(&self, input: &str) -> Result<String, Error> {
        let llm_config = &self.llm_config;
        let url = llm_config.full_url.as_str();
        let temperature = llm_config.temperature;

        let payload = if self.single_prompt {
            let prompt = format!("{}\n\n{}", self.prompt, input);
            json!({
                "contents": [{
                    "role": "user",
                    "parts": [{
                        "text": prompt
                    }]
                }],
                "generationConfig": {
                    //"candidateCount": 1u32,
                    "temperature": temperature,
                }
            })
        } else {
            let prompt = self.prompt.as_str();
            json!({
                "system_instruction": {
                    "parts": [{
                        "text": prompt
                    }]
                },
                "contents": [{
                    "role": "user",
                    "parts": [{
                        "text": input
                    }]
                }],
                "generationConfig": {
                    //"candidateCount": 1u32,
                    "temperature": temperature,
                }
            })
        };

        let response = self
            .client
            .post(url)
            .timeout(self.time_out)
            .json(&payload)
            .send()?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "Got failure HTTP response. status={}, body={}",
                response.status(),
                response.text().as_deref().unwrap_or("")
            ));
        }

        let mut decoded: GeminiChatResponse = response.json()?;
        if !decoded.candidates.is_empty() {
            let mut candidate = decoded.candidates.remove(0);
            if !candidate.content.parts.is_empty() {
                return Ok(candidate.content.parts.remove(0).text);
            }
        }
        Err(anyhow!("No text contained in response: {:?}", decoded))
    }

    // refer to https://platform.claude.com/docs/en/api/messages/create
    fn anthropic_interact(&self, input: &str) -> Result<String, Error> {
        let llm_config = &self.llm_config;
        let url = llm_config.full_url.as_str();
        let model_name = llm_config.model_name.as_str();
        let temperature = llm_config.temperature;

        let payload = if self.single_prompt {
            let prompt = format!("{}\n\n{}", self.prompt, input);
            json!({
                "model": model_name,
                "max_tokens": 65536u32,
                "messages": [{
                    "role": "user",
                    "content": prompt
                }],
                "stream": false,
                "temperature": temperature,
            })
        } else {
            let prompt = self.prompt.as_str();
            json!({
                "model": model_name,
                "max_tokens": 65536u32,
                "system": prompt,
                "messages": [{
                    "role": "user",
                    "content": input
                }],
                "stream": false,
                "temperature": temperature,
            })
        };

        let response = self
            .client
            .post(url)
            .timeout(self.time_out)
            .json(&payload)
            .send()?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "Got failure HTTP response. status={}, body={}",
                response.status(),
                response.text().as_deref().unwrap_or("")
            ));
        }

        let mut decoded: AnthropicChatResponse = response.json()?;
        if !decoded.content.is_empty() {
            return Ok(decoded.content.remove(0).text);
        }
        Err(anyhow!("No text contained in response: {:?}", decoded))
    }
}
