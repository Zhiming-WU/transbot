//! From the transbot crate you can build instance of translation robot to translate
//! documents (currently only HTML/EPUB is supported) by interact with an AI LLM
//! (Large Language Model).
//!
//! Currently resuming at middle of HTML is not supported, though resuming at middle
//! of EPUB is possible, starting from the chapter next to the last previously completely
//! translated chapter, with order defined by the spine, given that the generated
//! temporary file (named `<dest_file>.temp.<ext>`) is not removed.
//!
//! <br/>Below is an example of how to use the library crate.
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

use anyhow::Error;
use regex::Regex;
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::OnceLock;
use url::Url;

pub(crate) mod epub;
pub(crate) mod html1;
pub(crate) mod html2;
pub(crate) mod html3;
pub(crate) mod llm;

use llm::LlmConnector;

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
/// and 'http://localhost:13434/api/chat' is used if it's omitted. For custom providers,
/// the api sytle and the full service URL must be provided.
#[derive(Clone, Debug)]
pub enum LlmProvider {
    /// Self-defined provider.
    Custom {
        api_style: LlmApiStyle,
        full_url: String,
    },
    /// By ollama. Mostly running locally. Default URL is 'http://localhost:13434/api/chat'.
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
                        "Wrong custom provider format. It should be 'custom:<api_style>:<url>'"
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
    /// this crate itself.
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
    /// The selector selecting which elements in the HTML file to translate, by providing
    /// the tag names and maybe their attributes. The default is 'p,h1,h2,h3,li'. Tag names are
    /// separated by commas. As an example, 'p,h1,h2,h3,li,code[class=\"c\"]' also selects `code`
    /// elements having 'class' attribute set to 'c', which means comments in code blocks (however
    /// how code comments is defined is not common but specific to the HTML/EPUB file.
    /// Specify '*' to select all elements.
    /// And NOTICE that 'whole' means to pass the whole HTML to LLM to translate"
    pub html_elem_selector: Option<String>,
    /// The strategy to maintain the syntax defined by sub elements of selected elements in the
    /// document. If the 'html_elem_selector' field is 'whole', the syntax of the whole HTML file is
    /// maintaied by the LLM and this field is ignored.
    pub syntax_strategy: Option<SyntaxStrategy>,
    /// The prompt hint. The default is 'None' and the crate provide the default prompt, which
    /// is built from below template.
    /// "You are a professional translator. Translate the provided text [related to {prompt_topic}]
    /// into {dest_lang}. Strictly maintain the original HTML tags and HTML entities.
    /// Return the translated text only. {prompt_extra}"
    pub prompt_hint: Option<PromptHint>,
    /// Whether to print to the stdout the text passed to LLM and the result text gotten from it.
    /// It's mainly for checking during trying this crate on some LLM. The default is false.
    pub print_translating_text: Option<bool>,
    /// Whether to remove spaces between ASCII text (usually terminology) and the Chinese/Japanese/Korean
    /// text after translation. The spaces are usually added by the LLM during translation.
    /// The default is false.
    pub clean_cjk_ascii_spacing: Option<bool>,
}

impl TransConfig {
    /// Create a default TransConfig instance.
    pub fn new() -> Self {
        Self {
            dest_lang: None,
            html_elem_selector: None,
            syntax_strategy: None,
            prompt_hint: None,
            print_translating_text: None,
            clean_cjk_ascii_spacing: None,
        }
    }
    /// Set the destination language.
    pub fn set_dest_lang(&mut self, dest_lang: &str) -> &mut Self {
        self.dest_lang = Some(dest_lang.to_string());
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
    html_elem_selector: String,
    syntax_strategy: SyntaxStrategy,
    print_translating_text: bool,
    clean_spacing: bool,
}

fn verify_url(url_str: &str) -> Result<String, Error> {
    let url = Url::parse(url_str)?;
    Ok(url.to_string())
}

fn get_prompt(dest_lang: &str, prompt_hint: &Option<PromptHint>) -> String {
    let mut topic = "".to_string();
    let mut extra_prompt = "".to_string();
    if let Some(hint) = prompt_hint {
        if let Some(prompt) = hint.full_prompt.as_ref() {
            return prompt.to_owned();
        }
        if let Some(t) = hint.topic.as_ref() {
            topic = format!(" related to {}", t);
        }
        if let Some(e) = hint.extra_prompt.as_ref() {
            extra_prompt = e.to_owned();
        }
    }
    format!(
        "You are a professional translator. \
            Translate the provided text{} into {}. \
            Strictly maintain the original HTML tags and HTML entities. \
            Return the translated text only. {}",
        topic, dest_lang, extra_prompt
    )
}

pub(crate) fn get_extended_path<P: AsRef<Path>>(src_path: P, to_extend: &str) -> PathBuf {
    let path = src_path.as_ref().to_path_buf();
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let new_filename = format!("{}.{}.{}", stem, to_extend, ext);
    parent.join(new_filename)
}

/// The translation robot.
pub struct TransBot {
    trans_config: TransConfigInner,
    llm_interactor: LlmConnector,
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
                    None => String::from("http://localhost:13434/api/chat"),
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

        let trans_config_inner = TransConfigInner {
            dest_lang: trans_config
                .dest_lang
                .to_owned()
                .unwrap_or("Chinese(汉语)".into()),
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
        };

        let llm_interactor = LlmConnector::new(
            llm_config_inner,
            get_prompt(&trans_config_inner.dest_lang, &trans_config.prompt_hint),
            trans_config_inner.print_translating_text,
            trans_config_inner.clean_spacing,
        )?;

        Ok(Self {
            trans_config: trans_config_inner,
            llm_interactor,
        })
    }

    /// Set a new prompt.
    pub fn set_prompt(&mut self, prompt_hint: &PromptHint) {
        let hint = Some(prompt_hint.to_owned());
        self.llm_interactor
            .set_prompt(get_prompt(&self.trans_config.dest_lang, &hint));
    }

    /// Translate bytes of an HTML document.
    pub fn translate_html(&self, orig_html: &[u8]) -> Result<Vec<u8>, Error> {
        if self.trans_config.html_elem_selector.to_lowercase() == "whole" {
            let input = String::from_utf8_lossy(orig_html);
            let output = self.llm_interactor.interact(&input)?;
            return Ok(output.into());
        }
        match self.trans_config.syntax_strategy {
            SyntaxStrategy::MaintainedByTransBot => html1::translate_html(
                &self.llm_interactor,
                &self.trans_config.html_elem_selector,
                orig_html,
            ),
            SyntaxStrategy::MaintainedByLlm => html2::translate_html(
                &self.llm_interactor,
                &self.trans_config.html_elem_selector,
                orig_html,
            ),
            SyntaxStrategy::Stripped => html3::translate_html(
                &self.llm_interactor,
                &self.trans_config.html_elem_selector,
                orig_html,
            ),
        }
    }

    /// Translate an HTML file. If the 'dest_path' is passed as 'None',
    /// '<orig_filename>.transbot.<orig_ext>' is used, where <orig_filename>
    /// is the original file name and <orig_ext> is the original file extension.
    pub fn translate_html_file<P: AsRef<Path>>(
        &self,
        src_path: P,
        dest_path: Option<P>,
    ) -> Result<(), Error> {
        let input = std::fs::read(src_path.as_ref())?;
        let output = self.translate_html(&input)?;
        if let Some(dest) = dest_path {
            std::fs::write(dest, &output)?;
        } else {
            let dest = get_extended_path(src_path, "transbot");
            std::fs::write(dest, &output)?;
        }
        Ok(())
    }

    /// Translate an EPUB file. If the 'dest_path' is passed as 'None',
    /// '<orig_filename>.transbot.<orig_ext>' is used, where <orig_filename>
    /// is the original file name and <orig_ext> is the original file extension.
    pub fn translate_epub_file<P: AsRef<Path>>(
        &self,
        src_path: P,
        dest_path: Option<P>,
    ) -> Result<(), Error> {
        if let Some(dest) = dest_path {
            epub::epub(self, src_path, dest)
        } else {
            let dest = get_extended_path(src_path.as_ref(), "transbot");
            epub::epub(self, src_path, dest)
        }
    }

    pub(crate) fn get_llm_interactor(&self) -> &LlmConnector {
        &self.llm_interactor
    }
}

pub(crate) fn remove_boundary_spaces<'a>(text: &'a str) -> Cow<'a, str> {
    static RE_ASCII_NON: OnceLock<Regex> = OnceLock::new();
    static RE_NON_ASCII: OnceLock<Regex> = OnceLock::new();
    let re_ascii_non =
        RE_ASCII_NON.get_or_init(|| Regex::new(r"(\p{ASCII})\s+(\P{ASCII})").unwrap());
    let re_non_ascii =
        RE_NON_ASCII.get_or_init(|| Regex::new(r"(\P{ASCII})\s+(\p{ASCII})").unwrap());

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
