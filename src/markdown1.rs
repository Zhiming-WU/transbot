use anyhow::Error;
use pulldown_cmark::{CowStr, Event, Options, Parser, Tag, TagEnd};
use pulldown_cmark_to_cmark::cmark_with_options;
use serde::{Deserialize, Serialize};
use std::io::ErrorKind;

use crate::*;

#[derive(Default, Serialize, Deserialize)]
struct ProcessData {
    new_tag: bool,
    output_html: bool,
    depth: u32,
    chunk_index: usize,
    chunk_trans_index: usize,
    chunk_group_buf: String,
    chunk_group_vec: Vec<String>,
    chunk_vec: Vec<String>,
    html_index: usize,
    html_trans_index: usize,
    html_buf: String,
    html_vec: Vec<String>,
}

fn markdown_pass2(
    mut proc_data: ProcessData,
    orig_markdown: &str,
    trans_code: bool,
) -> Result<Vec<u8>, Error> {
    proc_data.depth = 0;
    proc_data.chunk_index = 0;
    proc_data.html_index = 0;

    let parser = get_cmark_parser(orig_markdown);
    let events = parser.filter_map(|event| match &event {
        Event::Text(text) => {
            if proc_data.depth > 0 && !text.trim().is_empty() {
                let index = proc_data.chunk_index;
                proc_data.chunk_index += 1;
                if index < proc_data.chunk_vec.len() {
                    let text = std::mem::take(&mut proc_data.chunk_vec[index]);
                    return Some(Event::Text(CowStr::from(text)));
                }
            }
            Some(event)
        }
        Event::Start(tag) => {
            match tag {
                Tag::Paragraph
                | Tag::Heading { .. }
                | Tag::List(_)
                | Tag::MetadataBlock(_)
                | Tag::Table(_)
                | Tag::FootnoteDefinition(_) => {
                    proc_data.depth += 1;
                }
                Tag::HtmlBlock => {
                    proc_data.output_html = true;
                }
                _ => {}
            }
            Some(event)
        }
        Event::End(tag) => {
            match tag {
                TagEnd::Paragraph
                | TagEnd::Heading { .. }
                | TagEnd::List(_)
                | TagEnd::MetadataBlock(_)
                | TagEnd::Table
                | TagEnd::FootnoteDefinition => {
                    if proc_data.depth > 0 {
                        proc_data.depth -= 1;
                    }
                }
                _ => {}
            }
            Some(event)
        }
        Event::Code(_text) => {
            let index = proc_data.chunk_index;
            proc_data.chunk_index += 1;
            if trans_code && index < proc_data.chunk_vec.len() {
                let translated = std::mem::take(&mut proc_data.chunk_vec[index]);
                return Some(Event::Code(CowStr::from(translated)));
            }
            Some(event)
        }
        Event::Html(_) => {
            if proc_data.output_html && proc_data.html_index < proc_data.html_vec.len() {
                proc_data.output_html = false;
                let html = std::mem::take(&mut proc_data.html_vec[proc_data.html_index]);
                proc_data.html_index += 1;
                return Some(Event::Html(CowStr::from(html)));
            }
            None
        }
        _ => Some(event),
    });
    let mut out = String::new();
    cmark_with_options(events, &mut out, markdown2::get_to_cmark_options())?;
    Ok(out.into_bytes())
}

fn get_cmark_parser(md_text: &str) -> Parser<'_> {
    Parser::new_ext(md_text, Options::all())
}

fn add_text_chunk(proc_data: &mut ProcessData, text: &str) {
    let index = proc_data.chunk_index;
    proc_data.chunk_index += 1;
    proc_data.chunk_vec.push(String::from(text));
    let sep = if proc_data.new_tag {
        proc_data.new_tag = false;
        "\n"
    } else {
        ""
    };
    proc_data.chunk_group_buf.push_str(&format!(
        "{}<{} id=\"{}\">{}</{}>",
        sep, SYNTAX_TAG, index, text, SYNTAX_TAG
    ));
}

fn markdown_pass1(orig_markdown: &str, chunk_size: usize) -> Result<ProcessData, Error> {
    let mut proc_data = ProcessData::default();
    let parser = get_cmark_parser(orig_markdown);
    let dummy = parser.map(|event| {
        match event {
            Event::Text(text) => {
                if proc_data.depth > 0 && !text.trim().is_empty() {
                    add_text_chunk(&mut proc_data, text.as_ref());
                }
            }
            Event::Start(
                Tag::Paragraph
                | Tag::Heading { .. }
                | Tag::List(_)
                | Tag::MetadataBlock(_)
                | Tag::Table(_)
                | Tag::FootnoteDefinition(_),
            ) => {
                proc_data.depth += 1;
            }
            Event::End(tag) => match tag {
                TagEnd::Paragraph
                | TagEnd::Heading { .. }
                | TagEnd::List(_)
                | TagEnd::MetadataBlock(_)
                | TagEnd::Table
                | TagEnd::FootnoteDefinition => {
                    if proc_data.depth > 0 {
                        proc_data.depth -= 1;
                    }
                    if proc_data.depth == 0 && proc_data.chunk_group_buf.len() > chunk_size {
                        let text = std::mem::take(&mut proc_data.chunk_group_buf);
                        proc_data.chunk_group_vec.push(text);
                    }
                }
                TagEnd::HtmlBlock => {
                    let html = std::mem::take(&mut proc_data.html_buf);
                    proc_data.html_vec.push(html);
                }
                _ => {}
            },
            // Add the code text to be translated to make translation accurate.
            // Whether to put in final translation result depends on config.
            Event::Code(text) => {
                add_text_chunk(&mut proc_data, text.as_ref());
            }
            Event::Html(text) => {
                proc_data.html_buf.push_str(text.as_ref());
            }
            _ => {}
        }
    });
    dummy.for_each(|_| {});
    if !proc_data.chunk_group_buf.is_empty() {
        let text = std::mem::take(&mut proc_data.chunk_group_buf);
        proc_data.chunk_group_vec.push(text);
    }
    Ok(proc_data)
}

fn translate_text(
    llm_interactor: &LlmConnector,
    text: &str,
    chunk_vec: &mut [String],
) -> Result<(), Error> {
    let result = llm_interactor.interact(text)?;
    html1::handle_tagged_result(&result, chunk_vec)?;
    Ok(())
}

fn serialize_proc_data<P: AsRef<Path>>(path: P, data: &ProcessData) -> Result<(), Error> {
    let bytes = bincode::serialize(data)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

pub(crate) fn translate_markdown<P: AsRef<Path>>(
    transbot: &TransBot,
    orig_markdown: &[u8],
    state_file_path: Option<P>,
) -> Result<Vec<u8>, Error> {
    let orig_text = String::from_utf8_lossy(orig_markdown);
    let chunk_size = transbot.trans_config.text_chunk_size;
    let mut proc_data = if let Some(path) = &state_file_path {
        match std::fs::read(path) {
            Ok(bytes) => {
                let decoded: ProcessData = bincode::deserialize(&bytes)?;
                decoded
            }
            Err(e) if e.kind() == ErrorKind::NotFound => markdown_pass1(&orig_text, chunk_size)?,
            Err(e) => {
                return Err(e.into());
            }
        }
    } else {
        markdown_pass1(&orig_text, chunk_size)?
    };

    if state_file_path.is_some() && transbot.is_interrupted() {
        return Err(TransBot::get_interrupted_error());
    }
    let mut get_progressed: bool;
    {
        let start_index = proc_data.chunk_trans_index;
        for index in start_index..proc_data.chunk_group_vec.len() {
            let chunk_group = std::mem::take(&mut proc_data.chunk_group_vec[index]);
            match translate_text(
                &transbot.llm_interactor,
                &chunk_group,
                &mut proc_data.chunk_vec,
            ) {
                Ok(_) => {
                    proc_data.chunk_trans_index = index + 1;
                }
                Err(e) => {
                    if let Some(path) = &state_file_path
                        && proc_data.chunk_trans_index > start_index
                    {
                        // put back the untranslated chunk group
                        proc_data.chunk_group_vec[index] = chunk_group;
                        let _ = serialize_proc_data(path, &proc_data);
                    }
                    return Err(e);
                }
            }
            if let Some(path) = &state_file_path
                && transbot.is_interrupted()
            {
                if proc_data.chunk_trans_index > start_index {
                    let _ = serialize_proc_data(path, &proc_data);
                }
                return Err(TransBot::get_interrupted_error());
            }
        }
        get_progressed = start_index != proc_data.chunk_trans_index;
    }
    {
        let start_index = proc_data.html_trans_index;
        for index in start_index..proc_data.html_vec.len() {
            let html = std::mem::take(&mut proc_data.html_vec[index]);
            match transbot.translate_html(html.as_bytes()) {
                Ok(translated) => {
                    proc_data.html_vec[index] = String::from_utf8_lossy(&translated).to_string();
                    proc_data.html_trans_index = index + 1;
                }
                Err(e) => {
                    if let Some(path) = &state_file_path
                        && proc_data.html_trans_index > start_index
                    {
                        // put back the untranslated chunk group
                        proc_data.html_vec[index] = html;
                        let _ = serialize_proc_data(path, &proc_data);
                    }
                    return Err(e);
                }
            }
            if let Some(path) = &state_file_path
                && transbot.is_interrupted()
            {
                if proc_data.html_trans_index > start_index {
                    let _ = serialize_proc_data(path, &proc_data);
                }
                return Err(TransBot::get_interrupted_error());
            }
        }
        get_progressed = get_progressed || start_index != proc_data.html_trans_index;
    }
    // Save the state file for possible failure before whole job done
    if let Some(path) = &state_file_path
        && get_progressed
    {
        let _ = serialize_proc_data(path, &proc_data);
    }

    markdown_pass2(
        proc_data,
        &orig_text,
        transbot.trans_config.trans_code_in_md,
    )
}
