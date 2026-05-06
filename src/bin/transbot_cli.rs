use anyhow::{Error, anyhow};
use clap::Parser;
use std::path::PathBuf;
use std::str::FromStr;
use transbot::{LlmConfig, LlmProvider, PromptHint, SyntaxStrategy, TransBot, TransConfig};

#[derive(Clone, Debug)]
enum FileFormat {
    HTML,
    EPUB,
}

impl FromStr for FileFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "html" => Ok(Self::HTML),
            "epub" => Ok(Self::EPUB),
            _ => Err(format!("Unsupported file format: {}", s)),
        }
    }
}

#[derive(clap::Args)]
struct PromptArgs {
    #[arg(long, help = "The topic to set in prompt")]
    prompt_topic: Option<String>,
    #[arg(long, help = "The extra text (such as glossary) to set in prompt")]
    prompt_extra: Option<String>,
    #[arg(
        long,
        help = "The full prompt text. If it's set, it replaces the whole default prompt set\n\
            by the program itself"
    )]
    full_prompt: Option<String>,
}

#[derive(Parser)]
#[command(name = "transbot")]
pub struct Cli {
    #[arg(short, long, help = "The input HTML/EPUB file path")]
    input_file: PathBuf,

    #[arg(
        short,
        long,
        help = "The output file path. The default is <orig_filename>.transbot.<orig_ext>, where <orig_filename>\n\
            is the original file name and <orig_ext> is the original file extension"
    )]
    output_file: Option<PathBuf>,

    #[arg(
        long,
        help = "The format of the input file. If omitted, determined by the file extension."
    )]
    file_format: Option<FileFormat>,

    #[arg(
        short,
        long,
        help = "The LLM provider name. It can be 'openai', 'gemini', 'anthropic', 'zhipu', 'deepseek', 'qwen',\n\
            'ollama[;url]' or 'custom;<api_style>;<url>', where url is the full URL of the LLM service,\n\
            and api_style can be 'ollama', 'openai', 'gemini', or 'anthropic'.\n\
            The default URL for ollama is 'http://localhost:11434/api/chat'."
    )]
    provider: LlmProvider,

    #[arg(short, long, help = "The LLM model name")]
    model_name: String,

    #[arg(short, long, help = "The LLM api key")]
    api_key: Option<String>,

    #[arg(long, help = "The LLM temperature. The default is 0.1")]
    temperature: Option<f64>,

    #[arg(
        long,
        help = "The time out of a single interaction with the LLM. The default is 300 seconds"
    )]
    llm_time_out: Option<u64>,

    #[command(flatten)]
    prompt_args: PromptArgs,

    #[arg(
        short,
        long,
        help = "The language to translate into. The default is Chinese"
    )]
    dest_lang: Option<String>,

    #[arg(
        long,
        help = "The selector selecting which elements in the HTML file to translate, by providing\n\
            the tag names and maybe their attributes. The default is 'p,h1,h2,h3,li'. Tag names are\n\
            separated by commas. As an example, 'p,h1,h2,h3,li,code[class=\"c1\"]' also selects `code`\n\
            elements having 'class' attribute set to 'c1', which means comments in code blocks (but how\n\
            code comments is defined is not common but specific to the HTML/EPUB file.\n\
            Specify '*' to select all elements.\n\
            And NOTICE that 'whole' means to pass the whole HTML to LLM to translate"
    )]
    html_elem_selector: Option<String>,

    #[arg(
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
        long,
        help = "Whether to print the text passed to LLM and the result text gotten from it. It's mainly for\n\
            checking during trying this program on some LLM. The default is false"
    )]
    print_translating_text: Option<bool>,

    #[arg(
        long,
        help = "Whether to remove spaces between ASCII text (usually terminology) and the Chinese/Japanese/Korean\n\
            text after translation. The spaces are usually added by the LLM during translation.\n\
            The default is false"
    )]
    clean_cjk_ascii_spacing: Option<bool>,
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
        html_elem_selector: cli.html_elem_selector,
        syntax_strategy: cli.syntax_strategy,
        prompt_hint: Some(prompt_hint),
        print_translating_text: cli.print_translating_text,
        clean_cjk_ascii_spacing: cli.clean_cjk_ascii_spacing,
    };

    let transbot = TransBot::new(&llm_config, &trans_config)?;
    match cli.file_format {
        Some(FileFormat::EPUB) => {
            transbot.translate_epub_file(&cli.input_file, cli.output_file.as_ref())?;
        }
        Some(FileFormat::HTML) => {
            transbot.translate_html_file(&cli.input_file, cli.output_file.as_ref())?;
        }
        _ => {
            if let Some(mime) = mime_guess::from_path(&cli.input_file).first() {
                if mime.essence_str().contains("epub") {
                    transbot.translate_epub_file(&cli.input_file, cli.output_file.as_ref())?;
                } else if mime.essence_str().contains("htm") {
                    transbot.translate_html_file(&cli.input_file, cli.output_file.as_ref())?;
                } else {
                    return Err(anyhow!("Unsupported input file format!"));
                }
            } else {
                return Err(anyhow!("Unkonwn input file format!"));
            }
        }
    }

    Ok(())
}
