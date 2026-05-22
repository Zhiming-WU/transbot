use anyhow::{Error, anyhow};
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use transbot::{
    DocFormat, LlmConfig, LlmProvider, PromptHint, SyntaxStrategy, TransBot, TransConfig,
};

#[derive(clap::Args)]
struct PromptArgs {
    #[arg(short = 't', long, help = "The topic to set in prompt")]
    prompt_topic: Option<String>,
    #[arg(
        short = 'e',
        long,
        help = "The extra text (such as glossary) to set in prompt"
    )]
    prompt_extra: Option<String>,
    #[arg(
        short = 'f',
        long,
        help = "The full prompt text. If it's set, it replaces the whole default prompt set\n\
            by the program itself"
    )]
    full_prompt: Option<String>,
}

#[derive(Parser)]
#[command(name = "transbot")]
pub struct Cli {
    #[arg(short = 'i', long, help = "The input HTML/EPUB file path")]
    input_file: PathBuf,

    #[arg(
        short = 'o',
        long,
        help = "The output file path. The default is <orig_filename>.transbot.<orig_ext>, where <orig_filename>\n\
            is the original file name and <orig_ext> is the original file extension"
    )]
    output_file: Option<PathBuf>,

    #[arg(
        short = 'F',
        long,
        help = "The format of the input file. It can be 'html', 'epub', 'md', or 'text'. If omitted, determined\n\
            by the file extension."
    )]
    file_format: Option<DocFormat>,

    #[arg(
        short = 'p',
        long,
        help = "The LLM provider name. It can be 'openai', 'gemini', 'anthropic', 'zhipu', 'deepseek', 'qwen',\n\
            'ollama[;url]' or 'custom;<api_style>;<url>', where url is the full URL of the LLM service,\n\
            and api_style can be 'ollama', 'openai', 'gemini', or 'anthropic'.\n\
            The default URL for ollama is 'http://localhost:11434/api/chat'."
    )]
    provider: LlmProvider,

    #[arg(short = 'm', long, help = "The LLM model name")]
    model_name: String,

    #[arg(short = 'a', long, help = "The LLM api key")]
    api_key: Option<String>,

    #[arg(long, help = "The LLM temperature. The default is 0.1")]
    temperature: Option<f64>,

    #[arg(
        short = 'T',
        long,
        help = "The time out of a single interaction with the LLM. The default is 300 seconds"
    )]
    llm_time_out: Option<u64>,

    #[command(flatten)]
    prompt_args: PromptArgs,

    #[arg(
        short = 'd',
        long,
        help = "The language to translate into. The default is Chinese"
    )]
    dest_lang: Option<String>,

    #[arg(
        short = 's',
        long,
        help = "Whether to use only single user prompt without system prompt.\n\
            The default is false"
    )]
    single_prompt: Option<bool>,

    #[arg(
        short = 'H',
        long,
        help = "The selector selecting which elements in the HTML file to translate, by providing\n\
            the tag names and maybe their attributes. The default is 'p,li,dd,h1,h2,h3,h4,h5,h6,title'.\n\
            Tag names are separated by commas. As an example, 'p,li,dd,h1,h2,h3,h4,h5,h6,title,code[class^=\"c\"]'\n\
            also selects `code` elements having 'class' attribute starting with 'c', which may mean comments\n\
            in code blocks (however how code comments is defined is not common but specific to the HTML/EPUB file).\n\
            Specify '*' to select all elements. For more complicated use, see the document at\n\
            https://docs.rs/lol_html/latest/lol_html/struct.Selector.html#supported-selector .\n\
            And NOTICE that 'whole' means to pass the whole HTML to LLM to translate"
    )]
    html_elem_selector: Option<String>,

    #[arg(
        short = 'S',
        long,
        help = "The syntax strategy during translation. It can be 'byllm', 'bytransbot' or 'stripped'.\n\
            The default is 'byllm'. This option is about how elements of non normal text, such as a link\n\
            or an '<em>', etc, are maintained. 'byllm' means they're maintained by LLM, 'bytransbot' means\n\
            they're maintained by this program, and 'stripped' means they're stripped. None of them is\n\
            ideally perfect.\n\
            It's IGNORED if the 'html_elem_selector' option is 'whole'"
    )]
    syntax_strategy: Option<SyntaxStrategy>,

    #[arg(
        short = 'P',
        long,
        help = "Whether to print the text passed to LLM and the result text gotten from it. It's mainly for\n\
            checking during trying this program on some LLM. The default is false"
    )]
    print_translating_text: Option<bool>,

    #[arg(
        short = 'C',
        long,
        help = "Whether to remove spaces between ASCII text (usually terminology) and the Chinese/Japanese/Korean\n\
            text after translation. The spaces are usually added by the LLM during translation.\n\
            The default is false"
    )]
    clean_cjk_ascii_spacing: Option<bool>,

    #[arg(
        short = 'w',
        long,
        help = "Whether to pass the the document to the LLM to translate, without parsing and splitting.\n\
            The default is false"
    )]
    whole_doc_to_llm: Option<bool>,

    #[arg(
        long,
        help = "Whether to translate code (usually defined by a ` pair. NOT the code block defined\n\
            by a ``` pair) in MarkDown. Make sense only for MarkDown documents if the 'syntax_strategy'\n\
            is 'bytransbot'. The default is false"
    )]
    trans_code_in_md: Option<bool>,

    #[arg(
        short = 'z',
        long,
        help = "The text size in characters to determine how long the text is sent to the LLM in some\n\
            situations. For example, in splitting long TEXT document to chunks to translate.\n\
            The default is 400"
    )]
    text_chunk_size: Option<usize>,
}

fn main() -> Result<(), Error> {
    let cli = Cli::parse();

    let llm_config = LlmConfig {
        model_name: cli.model_name,
        provider: cli.provider,
        api_key: cli.api_key,
        temperature: cli.temperature,
        time_out: cli.llm_time_out,
    };

    let prompt_hint = PromptHint {
        topic: cli.prompt_args.prompt_topic,
        extra_prompt: cli.prompt_args.prompt_extra,
        full_prompt: cli.prompt_args.full_prompt,
    };

    let trans_config = TransConfig {
        dest_lang: cli.dest_lang,
        single_prompt: cli.single_prompt,
        html_elem_selector: cli.html_elem_selector,
        syntax_strategy: cli.syntax_strategy,
        prompt_hint: Some(prompt_hint),
        print_translating_text: cli.print_translating_text,
        clean_cjk_ascii_spacing: cli.clean_cjk_ascii_spacing,
        whole_doc_to_llm: cli.whole_doc_to_llm,
        trans_code_in_md: cli.trans_code_in_md,
        text_chunk_size: cli.text_chunk_size,
    };

    let mut tbot = TransBot::new(&llm_config, &trans_config)?;
    tbot.set_resuming_support(true);
    let transbot = Arc::new(tbot);
    let transbot1 = transbot.clone();
    let _ = ctrlc::set_handler(move || {
        transbot1.set_interrupted();
    });
    match cli.file_format {
        Some(DocFormat::Epub) => {
            transbot.translate_epub_file(&cli.input_file, cli.output_file.as_ref())?;
        }
        Some(DocFormat::Html) => {
            transbot.translate_html_file(&cli.input_file, cli.output_file.as_ref())?;
        }
        Some(DocFormat::MarkDown) => {
            transbot.translate_markdown_file(&cli.input_file, cli.output_file.as_ref())?;
        }
        Some(DocFormat::Text) => {
            transbot.translate_text_file(&cli.input_file, cli.output_file.as_ref())?;
        }
        _ => {
            if let Some(mime) = mime_guess::from_path(&cli.input_file).first() {
                if mime.essence_str() == "application/epub+zip" {
                    transbot.translate_epub_file(&cli.input_file, cli.output_file.as_ref())?;
                } else if mime.essence_str().contains("htm") {
                    transbot.translate_html_file(&cli.input_file, cli.output_file.as_ref())?;
                } else if mime.essence_str() == "text/markdown" {
                    transbot.translate_markdown_file(&cli.input_file, cli.output_file.as_ref())?;
                } else if mime.essence_str() == "text/plain" {
                    transbot.translate_text_file(&cli.input_file, cli.output_file.as_ref())?;
                } else {
                    return Err(anyhow!(
                        "Unsupported input file format. [{}]",
                        mime.essence_str()
                    ));
                }
            } else {
                return Err(anyhow!("Unkonwn input file format!"));
            }
        }
    }

    Ok(())
}
