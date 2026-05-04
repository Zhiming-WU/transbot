use anyhow::Error;
use transbot::{LlmConfig, LlmProvider, PromptHint, SyntaxStrategy, TransBot, TransConfig};

fn main() -> Result<(), Error> {
    let llm_config = LlmConfig::new("translategemma:4b", LlmProvider::OLLAMA { full_url: None });
    let mut prompt_hint = PromptHint::new();
    prompt_hint.set_topic("Rust programming").set_extra_prompt(
        "Follow below term translation: \n\
        trait: 特型",
    );
    let mut trans_config = TransConfig::new();
    trans_config
        .set_dest_lang("Chinese")
        .set_html_elem_selector("p,h1,h2,h3,li,code[class=\"c\"]")
        .set_syntax_strategy(SyntaxStrategy::MaintainedByTransBot)
        .set_prompt_hint(prompt_hint)
        .set_clean_cjk_ascii_spacing(true)
        .set_print_translating_text(true);
    let transbot = TransBot::new(&llm_config, &trans_config)?;
    transbot.translate_epub_file("example.epub", None)
}
