A simple translation robot to translate HTML/EPUB/MarkDown/TEXT document based on LLMs.

For all supported formats supported except EPUB (but including HTML in EPUB), you can use
'whole_doc_to_llm' option to tell transbot to send the whole document to LLM to translate
without being parsed or splitted by transbot.

The syntax strategy makes sense only for HTML/MarkDown, and 'stripped' strategy is not supported
yet for MarkDown.

## For library use
See the API document at https://docs.rs/transbot. And see examples under `examples` sub directory.

Resuming is possible, and see the API document for how to handle interrupting and resuming.

## For command line use
See below `-h` output for its usage.

Resuming is possible. The state is saved into file(s) when an error occurs or when Ctrl+C is captured.
The state files are in `<dest_path>.temp[.x]` pattern, and no resuming is performed if they are removed.
And the syntax strategy (and also text chunk size in 'bytransbot' case) needs to be consistent for
resuming to work.

```text
Usage: transbot_cli [OPTIONS] --input-file <INPUT_FILE> --provider <PROVIDER> --model-name <MODEL_NAME>

Options:
  -i, --input-file <INPUT_FILE>
          The input HTML/EPUB file path
  -o, --output-file <OUTPUT_FILE>
          The output file path. The default is <orig_filename>.transbot.<orig_ext>, where <orig_filename>
          is the original file name and <orig_ext> is the original file extension
  -F, --file-format <FILE_FORMAT>
          The format of the input file. It can be 'html', 'epub', 'md', or 'text'. If omitted, determined
          by the file extension.
  -p, --provider <PROVIDER>
          The LLM provider name. It can be 'openai', 'gemini', 'anthropic', 'zhipu', 'deepseek', 'qwen',
          'ollama[;url]' or 'custom;<api_style>;<url>', where url is the full URL of the LLM service,
          and api_style can be 'ollama', 'openai', 'gemini', or 'anthropic'.
          The default URL for ollama is 'http://localhost:11434/api/chat'.
  -m, --model-name <MODEL_NAME>
          The LLM model name
  -a, --api-key <API_KEY>
          The LLM api key
      --temperature <TEMPERATURE>
          The LLM temperature. The default is 0.1
  -T, --llm-time-out <LLM_TIME_OUT>
          The time out of a single interaction with the LLM. The default is 300 seconds
  -t, --prompt-topic <PROMPT_TOPIC>
          The topic to set in prompt
  -e, --prompt-extra <PROMPT_EXTRA>
          The extra text (such as glossary) to set in prompt
  -f, --full-prompt <FULL_PROMPT>
          The full prompt text. If it's set, it replaces the whole default prompt set
          by the program itself
  -d, --dest-lang <DEST_LANG>
          The language to translate into. The default is Chinese
  -s, --single-prompt <SINGLE_PROMPT>
          Whether to use only single user prompt without system prompt.
          The default is false [possible values: true, false]
  -H, --html-elem-selector <HTML_ELEM_SELECTOR>
          The selector selecting which elements in the HTML file to translate, by providing
          the tag names and maybe their attributes. The default is 'p,h1,h2,h3,li'. Tag names are
          separated by commas. As an example, 'p,h1,h2,h3,li,code[class="c1"]' also selects `code`
          elements having 'class' attribute set to 'c1', which means comments in code blocks (but how
          code comments is defined is not common but specific to the HTML/EPUB file).
          Specify '*' to select all elements. For more complicated use, see the document at
          https://docs.rs/lol_html/latest/lol_html/struct.Selector.html#supported-selector .
          And NOTICE that 'whole' means to pass the whole HTML to LLM to translate
  -S, --syntax-strategy <SYNTAX_STRATEGY>
          The syntax strategy during translation. It can be 'byllm', 'bytransbot' or 'stripped'.
          The default is 'byllm'. This option is about how elements of non normal text, such as a link
          or an '<em>', etc, are maintained. 'byllm' means they're maintained by LLM, 'bytransbot' means
          they're maintained by this program, and 'stripped' means they're stripped. None of them is
          ideally perfect.
          It's IGNORED if the 'html_elem_selector' option is 'whole'
  -P, --print-translating-text <PRINT_TRANSLATING_TEXT>
          Whether to print the text passed to LLM and the result text gotten from it. It's mainly for
          checking during trying this program on some LLM. The default is false [possible values: true, false]
  -C, --clean-cjk-ascii-spacing <CLEAN_CJK_ASCII_SPACING>
          Whether to remove spaces between ASCII text (usually terminology) and the Chinese/Japanese/Korean
          text after translation. The spaces are usually added by the LLM during translation.
          The default is false [possible values: true, false]
  -w, --whole-doc-to-llm <WHOLE_DOC_TO_LLM>
          Whether to pass the the document to the LLM to translate, without parsing and splitting.
          The default is false [possible values: true, false]
      --trans-code-in-md <TRANS_CODE_IN_MD>
          Whether to translate code (usually defined by a ` pair. NOT the code block defined
          by a ``` pair) in MarkDown. Make sense only for MarkDown documents if the 'syntax_strategy'
          is 'bytransbot'. The default is false [possible values: true, false]
  -z, --text-chunk-size <TEXT_CHUNK_SIZE>
          The text size in characters to determine how long the text is sent to the LLM in some
          situations. For example, in splitting long TEXT document to chunks to translate.
          The default is 400
  -h, --help
          Print help
```
