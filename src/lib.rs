//! From the transbot crate you can build instance of translation robot to translate
//! documents (currently HTML/EPUB/MarkDown/TEXT is supported) by interact with an AI LLM
//! (Large Language Model).
//!
//! Resuming is possible. You need to call [TransBot::set_resuming_support] to enable it.
//! And to support saving middle state for later resuming when interrupting by Ctrl+C,
//! you need to capture the system signal and call [TransBot::set_interrupted] to notify
//! the library to know it so that it can save the middle state and quit the current job.
//! And notice below.
//! <br/>Interrupting check is not performed in middle of file IO or an interaction with the LLM.
//! but only between such actions.
//! <br/>Files like `<dest_path>.temp[.x]` are used to save the middle state, and no resuming is
//! performed if they are removed.
//! <br/>[TransConfig::syntax_strategy] (and also [TransConfig::text_chunk_size] in 'bytransbot' case)
//! needs to be consistent for resuming to work.
//!
//! For all supported formats supported except EPUB (but including HTML in EPUB), you can use
//! 'whole_doc_to_llm' option to tell transbot to send the whole document to LLM to translate
//! without being parsed or splitted by transbot.
//!
//! The syntax strategy makes sense only for HTML/MarkDown, and 'stripped' strategy is not supported
//! yet for MarkDown.
//!
//! Below is an example of how to use the library crate.
//! ```
//! use anyhow::Error;
//! use transbot::{LlmConfig, LlmProvider, PromptHint, SyntaxStrategy, TransBot, TransConfig};
//!
//! fn main() -> Result<(), Error> {
//!     let llm_config = LlmConfig::new("translategemma:4b", LlmProvider::OLLAMA { full_url: None });
//!     let mut prompt_hint = PromptHint::new();
//!     prompt_hint.set_topic("Rust programming").set_extra_prompt(
//!         "Follow below term translation: \n\
//!         trait: 特型",
//!     );
//!     let mut trans_config = TransConfig::new();
//!     trans_config
//!         .set_dest_lang("Chinese")
//!         .set_html_elem_selector("p,h1,h2,h3,li,code[class=\"c\"]")
//!         .set_syntax_strategy(SyntaxStrategy::MaintainedByTransBot)
//!         .set_prompt_hint(prompt_hint)
//!         .set_clean_cjk_ascii_spacing(true)
//!         .set_print_translating_text(true);
//!     let transbot = TransBot::new(&llm_config, &trans_config)?;
//!     transbot.translate_html_file("example.html", None)
//! }
//! ```

use anyhow::{Error, anyhow};
use regex::Regex;
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{
    OnceLock,
    atomic::{AtomicBool, Ordering},
};
use url::Url;

pub(crate) mod epub;
pub(crate) mod html1;
pub(crate) mod html2;
pub(crate) mod html3;
pub(crate) mod llm;
pub(crate) mod markdown1;
pub(crate) mod markdown2;
pub(crate) mod text;

use llm::LlmConnector;

#[derive(Clone, Debug)]
pub enum DocFormat {
    Html,
    Epub,
    MarkDown,
    Text,
}

impl FromStr for DocFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "html" => Ok(Self::Html),
            "epub" => Ok(Self::Html),
            "md" => Ok(Self::MarkDown),
            "text" => Ok(Self::Text),
            _ => Err(format!("Unsupported document format: {}", s)),
        }
    }
}

/// The API style of the LLM, which defines the message structure during interacting
/// with the LLM. Most LLM provides provide openai-compatible API (although its full
/// service URL is slightly differrent from the one for its native API). Please refer
/// to the API documents of your LLM provider if needed.
#[derive(Clone, Debug)]
pub enum LlmApiStyle {
    /// The sytle defined by ollama. (<https://docs.ollama.com/api/chat>)
    OLLAMA,
    /// The sytle defined by openai. (<https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create>)
    OPENAI,
    /// The sytle defined by gemini. (<https://ai.google.dev/gemini-api/docs/text-generation>)
    GEMINI,
    /// The sytle defined by anthropic. (<https://platform.claude.com/docs/en/api/messages/create>)
    ANTHROPIC,
}

impl FromStr for LlmApiStyle {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "ollama" => Ok(Self::OLLAMA),
            "openai" => Ok(Self::OPENAI),
            "gemini" => Ok(Self::GEMINI),
            "anthropic" => Ok(Self::ANTHROPIC),
            _ => Err(format!("Unsupported LLM API style: {}", s)),
        }
    }
}

/// The LLM provider. For ollama providers, an optional full service URL may be provided,
/// and 'http://localhost:11434/api/chat' is used if it's omitted. For custom providers,
/// the api sytle and the full service URL must be provided.
#[derive(Clone, Debug)]
pub enum LlmProvider {
    /// Self-defined provider.
    Custom {
        api_style: LlmApiStyle,
        full_url: String,
    },
    /// By ollama. Mostly running locally. Default URL is 'http://localhost:11434/api/chat'.
    OLLAMA { full_url: Option<String> },
    /// BY openpi. (<https://api.openai.com/v1/chat/completions>)
    OPENAI,
    /// BY gemini. (<https://generativelanguage.googleapis.com/v1beta/models>)
    GEMINI,
    /// BY anthropic. (<https://api.anthropic.com/v1/messages>)
    ANTHROPIC,
    /// BY zhipu. (<https://open.bigmodel.cn/api/paas/v4/chat/completions>)
    ZHIPU,
    /// BY deepseek. (<https://api.deepseek.com/chat/completions>)
    DEEPSEEK,
    /// BY qianwen. (<https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions>)
    QWEN,
}

impl FromStr for LlmProvider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(';').collect();
        let main_part = parts[0].to_lowercase();

        match main_part.as_str() {
            "openai" => Ok(Self::OPENAI),
            "gemini" => Ok(Self::GEMINI),
            "anthropic" => Ok(Self::ANTHROPIC),
            "zhipu" => Ok(Self::ZHIPU),
            "deepseek" => Ok(Self::DEEPSEEK),
            "qwen" => Ok(Self::QWEN),

            "ollama" => {
                let url = parts.get(1).map(|&u| u.to_string());
                Ok(Self::OLLAMA { full_url: url })
            }

            "custom" => {
                if parts.len() < 3 {
                    return Err(
                        "Wrong custom provider format. It should be 'custom;<api_style>;<url>'"
                            .into(),
                    );
                }
                let api_style = parts[1].parse::<LlmApiStyle>()?;
                let full_url = parts[2].to_string();
                Ok(Self::Custom {
                    api_style,
                    full_url,
                })
            }

            _ => Err(format!("Unkonwn provider: {}", main_part)),
        }
    }
}

/// The configuration for LLM interactions.
#[derive(Clone, Debug)]
pub struct LlmConfig {
    /// The model name.
    pub model_name: String,
    /// The LLM provider.
    pub provider: LlmProvider,
    /// The API key used by the LLM service for authentication. If it's omitted but required by the LLM,
    /// the interaction fails.
    pub api_key: Option<String>,
    /// The argument used by the LLM to control randomness during translation. The default is 0.1.
    pub temperature: Option<f64>,
    /// The time out of a single interaction with the LLM. The default is 300 seconds.
    pub time_out: Option<u64>,
}

impl LlmConfig {
    /// Create a LlmConfig instance with minimal arguments.
    pub fn new(model_name: &str, provider: LlmProvider) -> Self {
        Self {
            model_name: model_name.into(),
            provider,
            api_key: None,
            temperature: None,
            time_out: None,
        }
    }
    /// Set the API key.
    pub fn set_api_key(&mut self, api_key: &str) -> &mut Self {
        self.api_key = Some(api_key.to_string());
        self
    }
    /// Set the temperature.
    pub fn set_temperature(&mut self, temperature: f64) -> &mut Self {
        self.temperature = Some(temperature);
        self
    }
    /// Set the time out.
    pub fn set_time_out(&mut self, time_out: u64) -> &mut Self {
        self.time_out = Some(time_out);
        self
    }
}

#[derive(Clone, Debug)]
pub(crate) struct LlmConfigInner {
    pub model_name: String,
    pub full_url: String,
    pub api_style: LlmApiStyle,
    pub api_key: Option<String>,
    pub temperature: f64,
    pub time_out: u64,
}

/// The strategy to maintain the syntax defined by sub elements of selected elements in the
/// document. None of the options here is ideally perfect. Which one is suitable depends on
/// the LLM's strenth to maintain the HTML tags and how much LLM tokens you want to spend, and
/// whether losing the syntax is acceptable.
/// <br/>For example, for `See <a href="a_long_link">the blog</a> for details.` text
/// in the paragraph to translate, the behavior of each variant is explained in its document.
#[derive(Clone, Debug)]
pub enum SyntaxStrategy {
    /// The syntax is maintained by the LLM. In the example, the original
    /// `See <a href="a_long_link">the blog</a> for details.` is passed to the LLM and
    /// the response text is taken as the final translated text.
    /// <br/>Usually it consumes more tokens than other variants but maintained the syntax
    /// if the LLM is strong enough to correctly maintain the tags in the input.
    MaintainedByLlm,
    /// The crate tries to maintained the syntax. In the example, something like
    /// `<mytag id="1">See </mytag><mytag id="2">the blog</mytag><mytag id="3"> for details.</mytag>`
    /// is passed to the LLM and each part defined by 'mytag' is taken from the response text to
    /// create the translated text with the original link added back to the right part.
    /// <br/>It tries to maintain the syntax with less tokens but the result also relies on the LLM
    /// to correctly maintain the tags in the input.
    MaintainedByTransBot,
    /// The syntax is stripped. In the example, `See the blog for details.` is passed to the LLM and
    /// the response text is taken as the final translated text.
    /// <br/>It's best in text translation quality and consumes the least tokens, but the syntax is lost.
    Stripped,
}

impl FromStr for SyntaxStrategy {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "byllm" => Ok(Self::MaintainedByLlm),
            "bytransbot" => Ok(Self::MaintainedByTransBot),
            "stripped" => Ok(Self::Stripped),
            _ => Err(format!("Unsupported syntax strategy: {}", s)),
        }
    }
}

/// The prompt hint.
#[derive(Clone, Debug)]
pub struct PromptHint {
    /// The topic of the document to translate
    pub topic: Option<String>,
    /// The extra instructions to ask the LLM to follow, like how to translate glossary, etc.
    pub extra_prompt: Option<String>,
    /// The full prompt. If it's provided, it replaces the whole default prompt set by
    /// this crate itself. For translategemma model, it's a good idea to refer to its prompt
    /// guide at <https://www.ollama.com/library/translategemma>.
    pub full_prompt: Option<String>,
}

impl PromptHint {
    /// Create a default PromptHint instance.
    pub fn new() -> Self {
        Self {
            topic: None,
            extra_prompt: None,
            full_prompt: None,
        }
    }
    /// Set the topic.
    pub fn set_topic(&mut self, topic: &str) -> &mut Self {
        self.topic = Some(topic.to_string());
        self
    }
    /// Set the extra prompt.
    pub fn set_extra_prompt(&mut self, extra_prompt: &str) -> &mut Self {
        self.extra_prompt = Some(extra_prompt.to_string());
        self
    }
    /// Set the full prompt.
    pub fn set_full_prompt(&mut self, full_prompt: &str) -> &mut Self {
        self.full_prompt = Some(full_prompt.to_string());
        self
    }
}

impl Default for PromptHint {
    fn default() -> Self {
        Self::new()
    }
}

/// The configuration for translation.
#[derive(Clone, Debug)]
pub struct TransConfig {
    /// The destination language to translate into. The default is Chinese.
    pub dest_lang: Option<String>,
    /// Whether to use only single user prompt without system prompt. The default is false.
    /// If set to true, the translation instruction is put in the user prompt, and the text
    /// to be translated is appended after it with two blank lines added between them.
    /// If set to false, the translation instruction is put in the system prompt, and the
    /// text to be translated is put in the user prompt.
    pub single_prompt: Option<bool>,
    /// The selector selecting which elements in the HTML file to translate, by providing
    /// the tag names and maybe their attributes. The default is 'p,h1,h2,h3,li'. Tag names are
    /// separated by commas. As an example, 'p,h1,h2,h3,li,code[class=\"c\"]' also selects 'code'
    /// elements having 'class' attribute set to 'c', which means comments in code blocks (however
    /// how code comments is defined is not common but specific to the HTML/EPUB file.
    /// Specify '*' to select all elements. <br/>For more complicated use, see the document at
    /// <https://docs.rs/lol_html/latest/lol_html/struct.Selector.html#supported-selector>.
    /// <br/>And NOTICE that 'whole' means to pass the whole HTML to LLM to translate.
    pub html_elem_selector: Option<String>,
    /// The strategy to maintain the syntax defined by sub elements of selected elements in the
    /// document. If the 'whole_doc_to_llm' field is true or the 'html_elem_selector' field is 'whole',
    /// the syntax of the whole HTML file is maintaied by the LLM and this field is ignored.
    pub syntax_strategy: Option<SyntaxStrategy>,
    /// The prompt hint. The default is 'None' and the crate provide the default prompt, which
    /// is built from template something like below.
    /// <br/>"You are a professional translator specializing in translating text into {dest_lang}.
    /// Your goal is to accurately convey the meaning and nuances of the original text
    /// while adhering to {dest_lang} grammar, vocabulary, and cultural sensitivities.
    /// Produce only the {dest_lang} translation, without any additional explanations or commentary.
    /// Strictly maintain the original HTML tags and HTML entities.[ {extra_prompt}\n]
    /// Please translate the provided [{prompt_topic} related ]text into {dest_lang}."
    pub prompt_hint: Option<PromptHint>,
    /// Whether to print to the stdout the text passed to LLM and the result text gotten from it.
    /// It's mainly for checking during trying this crate on some LLM. The default is false.
    pub print_translating_text: Option<bool>,
    /// Whether to remove spaces between ASCII text (usually terminology) and the Chinese/Japanese/Korean
    /// text after translation. The spaces are usually added by the LLM during translation.
    /// The default is false.
    pub clean_cjk_ascii_spacing: Option<bool>,
    /// Whether to pass the the document (like the HTML (include the HTML in an EPUB), the MarkDown,
    /// The TEXT, etc. but NOT the EPUB) to the LLM to translate, without parsing and splitting.
    /// The default is false.
    pub whole_doc_to_llm: Option<bool>,
    /// Whether to translate code (usually defined by a \` pair. NOT the code block defined by a \`\`\` pair)
    /// in MarkDown. Make sense only for MarkDown documents if the 'syntax_strategy' is 'bytransbot'.
    /// The default is false.
    pub trans_code_in_md: Option<bool>,
    /// The text size in characters to determine how long the text is sent to the LLM in some situations.
    /// For example, in splitting long TEXT document to chunks to translate. The default is 400.
    pub text_chunk_size: Option<usize>,
}

impl TransConfig {
    /// Create a default TransConfig instance.
    pub fn new() -> Self {
        Self {
            dest_lang: None,
            single_prompt: None,
            html_elem_selector: None,
            syntax_strategy: None,
            prompt_hint: None,
            print_translating_text: None,
            clean_cjk_ascii_spacing: None,
            whole_doc_to_llm: None,
            trans_code_in_md: None,
            text_chunk_size: None,
        }
    }
    /// Set the destination language.
    pub fn set_dest_lang(&mut self, dest_lang: &str) -> &mut Self {
        self.dest_lang = Some(dest_lang.to_string());
        self
    }
    /// Set whether to use only single user prompt without system prompt.
    pub fn set_single_prompt(&mut self, single_prompt: bool) -> &mut Self {
        self.single_prompt = Some(single_prompt);
        self
    }
    /// Set the HTML element selector.
    pub fn set_html_elem_selector(&mut self, html_elem_selector: &str) -> &mut Self {
        self.html_elem_selector = Some(html_elem_selector.to_string());
        self
    }
    /// Set the syntax strategy.
    pub fn set_syntax_strategy(&mut self, syntax_strategy: SyntaxStrategy) -> &mut Self {
        self.syntax_strategy = Some(syntax_strategy);
        self
    }
    /// Set the prompt hint.
    pub fn set_prompt_hint(&mut self, prompt_hint: PromptHint) -> &mut Self {
        self.prompt_hint = Some(prompt_hint);
        self
    }
    /// Set whether to print the translating text.
    pub fn set_print_translating_text(&mut self, print_translating_text: bool) -> &mut Self {
        self.print_translating_text = Some(print_translating_text);
        self
    }
    /// Set whether to clean extra spaces.
    pub fn set_clean_cjk_ascii_spacing(&mut self, clean_cjk_ascii_spacing: bool) -> &mut Self {
        self.clean_cjk_ascii_spacing = Some(clean_cjk_ascii_spacing);
        self
    }
}

impl Default for TransConfig {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) struct TransConfigInner {
    dest_lang: String,
    single_prompt: bool,
    html_elem_selector: String,
    syntax_strategy: SyntaxStrategy,
    print_translating_text: bool,
    clean_spacing: bool,
    whole_doc_to_llm: bool,
    trans_code_in_md: bool,
    text_chunk_size: usize,
    syntax_tag: String,
}

fn verify_url(url_str: &str) -> Result<String, Error> {
    let url = Url::parse(url_str)?;
    Ok(url.to_string())
}

fn get_prompt(dest_lang: &str, prompt_hint: &Option<PromptHint>, single_prompt: bool) -> String {
    let mut topic = "".to_string();
    let mut extra_prompt = "".to_string();
    if let Some(hint) = prompt_hint {
        if let Some(prompt) = hint.full_prompt.as_ref() {
            return prompt.to_owned();
        }
        if let Some(t) = hint.topic.as_ref() {
            topic = format!("The topic of the text is '{}'.\n", t);
        }
        if let Some(e) = hint.extra_prompt.as_ref() {
            extra_prompt = format!("{}\n", e);
        }
    }
    let translate_request = if single_prompt {
        format!("Please translate below text into {}:", dest_lang)
    } else {
        format!("Please translate the provided text into {}.", dest_lang)
    };
    format!(
        "You are a professional translator. Your task is to translate the provided text into {}.\n\
            {}\
            Strictly maintain the original format, the HTML tags, their attributes and the HTML entities.\n\
            Produce only the translation text, without any additional explanations or commentary.\n\
            {}\
            {}",
        dest_lang, topic, extra_prompt, &translate_request,
    )
}

pub(crate) fn get_extended_path<P: AsRef<Path>>(
    src_path: P,
    to_extend: &str,
    at_end: bool,
) -> PathBuf {
    let path = src_path.as_ref().to_path_buf();
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let new_filename = if at_end {
        let filename = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        format!("{}.{}", filename, to_extend,)
    } else {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        format!("{}.{}.{}", stem, to_extend, ext)
    };
    parent.join(new_filename)
}

/// The translation robot.
pub struct TransBot {
    trans_config: TransConfigInner,
    llm_interactor: LlmConnector,
    resuming_enabled: bool,
    is_interrupted: AtomicBool,
}

impl TransBot {
    /// Create a TransBot instance.
    pub fn new(llm_config: &LlmConfig, trans_config: &TransConfig) -> Result<Self, Error> {
        let mut llm_config_inner = LlmConfigInner {
            model_name: llm_config.model_name.to_owned(),
            api_style: match &llm_config.provider {
                LlmProvider::Custom {
                    full_url: _,
                    api_style: style,
                } => style.to_owned(),
                LlmProvider::OLLAMA { full_url: _ } => LlmApiStyle::OLLAMA,
                LlmProvider::GEMINI => LlmApiStyle::GEMINI,
                LlmProvider::ANTHROPIC => LlmApiStyle::ANTHROPIC,
                _ => LlmApiStyle::OPENAI,
            },
            full_url: match &llm_config.provider {
                LlmProvider::Custom {
                    full_url: url,
                    api_style: _,
                } => verify_url(url)?,
                LlmProvider::OLLAMA { full_url: url } => match url {
                    Some(url) => verify_url(url)?,
                    None => String::from("http://localhost:11434/api/chat"),
                },
                LlmProvider::GEMINI => {
                    String::from("https://generativelanguage.googleapis.com/v1beta/models")
                }
                LlmProvider::OPENAI => String::from("https://api.openai.com/v1/chat/completions"),
                LlmProvider::ANTHROPIC => String::from("https://api.anthropic.com/v1/messages"),
                LlmProvider::ZHIPU => {
                    String::from("https://open.bigmodel.cn/api/paas/v4/chat/completions")
                }
                LlmProvider::DEEPSEEK => String::from("https://api.deepseek.com/chat/completions"),
                LlmProvider::QWEN => String::from(
                    "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions",
                ),
            },
            api_key: llm_config.api_key.to_owned(),
            temperature: llm_config.temperature.unwrap_or(0.1),
            time_out: llm_config.time_out.unwrap_or(300),
        };
        if let LlmApiStyle::GEMINI = &llm_config_inner.api_style {
            llm_config_inner.full_url = verify_url(&format!(
                "{}/{}:generateContent",
                llm_config_inner.full_url, llm_config_inner.model_name
            ))?;
        }

        let syntax_tag = std::env::var("TRANSBOT_SYNTAX_TAG").unwrap_or(String::from(SYNTAX_TAG));
        let trans_config_inner = TransConfigInner {
            dest_lang: trans_config
                .dest_lang
                .to_owned()
                .unwrap_or("Chinese(zh-Hans)".into()),
            single_prompt: trans_config.single_prompt.unwrap_or(false),
            html_elem_selector: trans_config
                .html_elem_selector
                .to_owned()
                .unwrap_or("p,h1,h2,h3,li".into()),
            syntax_strategy: trans_config
                .syntax_strategy
                .to_owned()
                .unwrap_or(SyntaxStrategy::MaintainedByLlm),
            print_translating_text: trans_config.print_translating_text.unwrap_or(false),
            clean_spacing: trans_config.clean_cjk_ascii_spacing.unwrap_or(false),
            whole_doc_to_llm: trans_config.whole_doc_to_llm.unwrap_or(false),
            trans_code_in_md: trans_config.trans_code_in_md.unwrap_or(false),
            text_chunk_size: trans_config.text_chunk_size.unwrap_or(400),
            syntax_tag,
        };

        let llm_interactor = LlmConnector::new(
            llm_config_inner,
            trans_config_inner.single_prompt,
            get_prompt(
                &trans_config_inner.dest_lang,
                &trans_config.prompt_hint,
                trans_config_inner.single_prompt,
            ),
            trans_config_inner.print_translating_text,
            trans_config_inner.clean_spacing,
        )?;

        Ok(Self {
            trans_config: trans_config_inner,
            llm_interactor,
            resuming_enabled: false,
            is_interrupted: AtomicBool::new(false),
        })
    }

    /// Enable or disable the resuming support.
    pub fn set_resuming_support(&mut self, enabled: bool) {
        self.resuming_enabled = enabled;
    }

    /// Notify the library that the program is interrupted.
    pub fn set_interrupted(&self) {
        self.is_interrupted.store(true, Ordering::Release);
    }

    pub(crate) fn is_interrupted(&self) -> bool {
        self.is_interrupted.load(Ordering::Acquire)
    }

    pub(crate) fn get_interrupted_error() -> Error {
        anyhow!("The translation job is interrupted.")
    }

    /// Set a new prompt.
    pub fn set_prompt(&mut self, prompt_hint: &PromptHint) {
        let hint = Some(prompt_hint.to_owned());
        self.llm_interactor.set_prompt(get_prompt(
            &self.trans_config.dest_lang,
            &hint,
            self.trans_config.single_prompt,
        ));
    }

    /// Translate bytes of an HTML document, no resuming support.
    pub fn translate_html(&self, orig_html: &[u8]) -> Result<Vec<u8>, Error> {
        self.translate_bytes::<&str>(DocFormat::Html, orig_html, None)
    }

    /// Translate bytes of an HTML document. For resuming support, you should enable it (via
    /// [TransBot::set_resuming_support]) and pass 'state_file_path' with a 'Some' value.
    pub fn translate_html_resumable<P: AsRef<Path>>(
        &self,
        orig_html: &[u8],
        state_file_path: Option<P>,
    ) -> Result<Vec<u8>, Error> {
        self.translate_bytes(DocFormat::Html, orig_html, state_file_path.as_ref())
    }

    /// Translate an HTML file. If 'dest_path' is 'None', '<src_filename>.transbot.<src_ext>' is used.
    /// If resuming is enabled (via [TransBot::set_resuming_support]), the state file path '<dest_path>.temp'
    /// is used for resuming support.
    pub fn translate_html_file<P: AsRef<Path>>(
        &self,
        src_path: P,
        dest_path: Option<P>,
    ) -> Result<(), Error> {
        self.translate_file(DocFormat::Html, src_path, dest_path)
    }

    /// Translate an EPUB file. If 'dest_path' is 'None', '<src_filename>.transbot.<src_ext>' is used.
    pub fn translate_epub_file<P: AsRef<Path>>(
        &self,
        src_path: P,
        dest_path: Option<P>,
    ) -> Result<(), Error> {
        if let Some(dest) = dest_path {
            epub::epub(self, src_path, dest)
        } else {
            let dest = get_extended_path(src_path.as_ref(), "transbot", false);
            epub::epub(self, src_path, dest)
        }
    }

    /// Translate bytes of an MarkDown document. For resuming support, you should enable it (via
    /// [TransBot::set_resuming_support]) and pass 'state_file_path' with a 'Some' value.
    pub fn translate_markdown<P: AsRef<Path>>(
        &self,
        orig_markdown: &[u8],
        state_file_path: Option<P>,
    ) -> Result<Vec<u8>, Error> {
        self.translate_bytes(DocFormat::MarkDown, orig_markdown, state_file_path.as_ref())
    }

    /// Translate a MarkDown file. If 'dest_path' is 'None', '<src_filename>.transbot.<src_ext>' is used.
    /// If resuming is enabled (via [TransBot::set_resuming_support]), the state file path '<dest_path>.temp'
    /// is used for resuming support.
    pub fn translate_markdown_file<P: AsRef<Path>>(
        &self,
        src_path: P,
        dest_path: Option<P>,
    ) -> Result<(), Error> {
        self.translate_file(DocFormat::MarkDown, src_path, dest_path)
    }

    /// Translate bytes of an TEXT document. For resuming support, you should enable it (via
    /// [TransBot::set_resuming_support]) and pass 'state_file_path' with a 'Some' value.
    pub fn translate_text<P: AsRef<Path>>(
        &self,
        orig_markdown: &[u8],
        state_file_path: Option<P>,
    ) -> Result<Vec<u8>, Error> {
        self.translate_bytes(DocFormat::Text, orig_markdown, state_file_path.as_ref())
    }

    /// Translate a TEXT file. If 'dest_path' is 'None', '<src_filename>.transbot.<src_ext>' is used.
    /// If resuming is enabled (via [TransBot::set_resuming_support]), the state file path '<dest_path>.temp'
    /// is used for resuming support.
    pub fn translate_text_file<P: AsRef<Path>>(
        &self,
        src_path: P,
        dest_path: Option<P>,
    ) -> Result<(), Error> {
        self.translate_file(DocFormat::Text, src_path, dest_path)
    }

    fn translate_bytes_no_delete<P: AsRef<Path>>(
        &self,
        format: DocFormat,
        orig_doc: &[u8],
        state_file_path: Option<P>,
    ) -> Result<Vec<u8>, Error> {
        let mut whole_doc_to_llm = self.trans_config.whole_doc_to_llm;
        if let DocFormat::Html = format
            && self.trans_config.html_elem_selector.to_lowercase() == "whole"
        {
            whole_doc_to_llm = true;
        }
        if whole_doc_to_llm {
            let input = String::from_utf8_lossy(orig_doc);
            let output = self.llm_interactor.interact(&input)?;
            return Ok(output.into());
        }
        match format {
            DocFormat::Html => match self.trans_config.syntax_strategy {
                SyntaxStrategy::MaintainedByTransBot => {
                    html1::translate_html(self, orig_doc, state_file_path)
                }
                SyntaxStrategy::MaintainedByLlm => {
                    html2::translate_html(self, orig_doc, state_file_path)
                }
                SyntaxStrategy::Stripped => html3::translate_html(self, orig_doc, state_file_path),
            },
            DocFormat::MarkDown => match self.trans_config.syntax_strategy {
                SyntaxStrategy::MaintainedByTransBot => {
                    markdown1::translate_markdown(self, orig_doc, state_file_path)
                }
                SyntaxStrategy::MaintainedByLlm => {
                    markdown2::translate_markdown(self, orig_doc, state_file_path)
                }
                SyntaxStrategy::Stripped => Err(anyhow!(
                    "Syntax strategy 'stripped' is not supported yet for MarkDown files."
                )),
            },
            DocFormat::Text => text::translate_text(self, orig_doc, state_file_path),
            _ => Err(anyhow!("Unexpected format.")),
        }
    }

    fn translate_bytes<P: AsRef<Path>>(
        &self,
        format: DocFormat,
        orig_doc: &[u8],
        state_file_path: Option<P>,
    ) -> Result<Vec<u8>, Error> {
        let state_file_path = if !self.resuming_enabled {
            None
        } else {
            state_file_path
        };
        let out = self.translate_bytes_no_delete(format, orig_doc, state_file_path.as_ref())?;
        if self.resuming_enabled
            && let Some(path) = state_file_path
        {
            let _ = std::fs::remove_file(path);
        }
        Ok(out)
    }

    fn translate_file<P: AsRef<Path>>(
        &self,
        format: DocFormat,
        src_path: P,
        dest_path: Option<P>,
    ) -> Result<(), Error> {
        let input = std::fs::read(src_path.as_ref())?;
        let dest = if let Some(dest) = dest_path {
            dest.as_ref().to_path_buf()
        } else {
            get_extended_path(src_path, "transbot", false)
        };
        let state_file_path = if self.resuming_enabled {
            Some(get_extended_path(&dest, "temp", true))
        } else {
            None
        };
        let output = self.translate_bytes_no_delete(format, &input, state_file_path.as_ref())?;
        std::fs::write(&dest, &output)?;
        if self.resuming_enabled
            && let Some(path) = state_file_path.as_ref()
        {
            let _ = std::fs::remove_file(path);
        }
        Ok(())
    }

    pub(crate) fn get_llm_interactor(&self) -> &LlmConnector {
        &self.llm_interactor
    }
}

pub(crate) fn remove_boundary_spaces<'a>(text: &'a str) -> Cow<'a, str> {
    static RE_ASCII_NON: OnceLock<Regex> = OnceLock::new();
    static RE_NON_ASCII: OnceLock<Regex> = OnceLock::new();
    // Exclude '#','-','*', since in MarkDown, space in "# XX", "- XX", "* XX" makes sense (to form
    // heading or list item).
    let re_ascii_non =
        RE_ASCII_NON.get_or_init(|| Regex::new("([^\\P{ASCII}#\\-*]) +(\\P{ASCII})").unwrap());
    let re_non_ascii =
        RE_NON_ASCII.get_or_init(|| Regex::new(r"(\P{ASCII}) +(\p{ASCII})").unwrap());

    let step1 = re_ascii_non.replace_all(text, "$1$2");
    match step1 {
        Cow::Borrowed(b) => re_non_ascii.replace_all(b, "$1$2"),
        Cow::Owned(s) => {
            let step2 = re_non_ascii.replace_all(&s, "$1$2");
            match step2 {
                Cow::Borrowed(_) => Cow::Owned(s),
                Cow::Owned(s2) => Cow::Owned(s2),
            }
        }
    }
}

pub(crate) const SYNTAX_TAG: &str = "a";

fn count_leading_newline(text: &str) -> usize {
    let mut result = 0usize;
    let bytes = text.as_bytes();
    while result < bytes.len() && bytes[result] == b'\n' {
        result += 1;
    }
    result
}

fn count_tailing_newline(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut idx = bytes.len();
    while idx > 0 && bytes[idx - 1] == b'\n' {
        idx -= 1;
    }
    bytes.len() - idx
}

pub(crate) fn restore_triming_newlines(dest: &mut String, orig: &str) {
    let mut leading = String::new();
    for _ in 0..count_leading_newline(orig) {
        leading.push('\n');
    }
    if !leading.is_empty() {
        dest.insert_str(0, leading.as_str());
    }
    for _ in 0..count_tailing_newline(orig) {
        dest.push('\n');
    }
}
