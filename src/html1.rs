use anyhow::Error;
use lol_html::{HtmlRewriter, Settings, element, end_tag, text};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::io::ErrorKind;
use std::rc::Rc;

use crate::*;

#[derive(Default, Serialize, Deserialize)]
struct ProcessData {
    new_tag: bool,
    depth: u32,
    parse_index: usize,
    trans_index: usize,
    chunk_group_buf: String,
    chunk_group_vec: Vec<String>,
    chunk_vec: Vec<String>,
}

pub(crate) fn handle_tagged_result(
    text: &str,
    chunk_vec: &mut [String],
    syntax_tag: &str,
) -> Result<(), Error> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);

    let len = chunk_vec.len();
    let mut buf = Vec::new();
    let mut index = len;
    let mut has_text = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if e.name().as_ref() == syntax_tag.as_bytes()
                    && let Ok(Some(id)) = e.try_get_attribute("id")
                    && let Ok(id) = String::from_utf8_lossy(&id.value).parse::<usize>()
                {
                    index = id;
                    has_text = false;
                }
            }
            Ok(Event::Text(e)) => {
                has_text = true;
                if index < len {
                    chunk_vec[index] = String::from_utf8_lossy(e.as_ref()).into_owned();
                }
            }
            Ok(Event::End(_)) => {
                if !has_text && index < len {
                    chunk_vec[index] = String::new();
                }
                index = len;
            }
            Ok(Event::Eof) => break,
            _ => (),
        }
        buf.clear();
    }

    Ok(())
}

fn html_pass2(
    data: Rc<RefCell<ProcessData>>,
    elem_selector: &str,
    orig_html: &[u8],
) -> Result<Vec<u8>, Error> {
    {
        let mut proc_data = data.borrow_mut();
        proc_data.depth = 0;
        proc_data.parse_index = 0;
    }
    let data1 = data.clone();
    let data2 = data.clone();
    let mut out = Vec::<u8>::new();
    let mut rewriter = HtmlRewriter::new(
        Settings {
            element_content_handlers: vec![
                element!(elem_selector, move |el| {
                    if el.is_self_closing() {
                        return Ok(());
                    }
                    let data3 = data1.clone();
                    {
                        let mut proc_data = data1.borrow_mut();
                        proc_data.depth += 1;
                    }
                    el.on_end_tag(end_tag!(move |_end| {
                        let mut proc_data = data3.borrow_mut();
                        if proc_data.depth > 0 {
                            proc_data.depth -= 1;
                        }
                        Ok(())
                    }))
                }),
                text!("*", move |text_chunk| {
                    let mut proc_data = data2.borrow_mut();
                    if proc_data.depth > 0 {
                        let chunk = text_chunk.as_str();
                        if !chunk.trim().is_empty() {
                            let index = proc_data.parse_index;
                            proc_data.parse_index += 1;
                            if index < proc_data.chunk_vec.len() {
                                let translated = std::mem::take(&mut proc_data.chunk_vec[index]);
                                text_chunk.replace(
                                    &translated,
                                    lol_html::html_content::ContentType::Text,
                                );
                            }
                        }
                    }
                    Ok(())
                }),
            ],
            ..Settings::default()
        },
        |c: &[u8]| {
            out.extend_from_slice(c);
        },
    );
    rewriter.write(orig_html)?;
    rewriter.end()?;
    Ok(out)
}

fn html_pass1(
    elem_selector: &str,
    orig_html: &[u8],
    chunk_size: usize,
    syntax_tag: &str,
) -> Result<Rc<RefCell<ProcessData>>, Error> {
    let data = Rc::new(RefCell::new(ProcessData::default()));
    let data1 = data.clone();
    let data2 = data.clone();
    let mut rewriter = HtmlRewriter::new(
        Settings {
            element_content_handlers: vec![
                element!(elem_selector, move |el| {
                    if el.is_self_closing() {
                        return Ok(());
                    }
                    let data3 = data1.clone();
                    {
                        let mut proc_data = data1.borrow_mut();
                        proc_data.depth += 1;
                        proc_data.new_tag = true;
                    }
                    el.on_end_tag(end_tag!(move |_end| {
                        let mut proc_data = data3.borrow_mut();
                        if proc_data.depth > 0 {
                            proc_data.depth -= 1;
                        }
                        if proc_data.depth == 0 && proc_data.chunk_group_buf.len() > chunk_size {
                            let chunk = std::mem::take(&mut proc_data.chunk_group_buf);
                            proc_data.chunk_group_vec.push(chunk);
                        }
                        Ok(())
                    }))
                }),
                text!("*", move |text_chunk| {
                    let mut proc_data = data2.borrow_mut();
                    if proc_data.depth > 0 {
                        let chunk = text_chunk.as_str();
                        if !chunk.trim().is_empty() {
                            let index = proc_data.parse_index;
                            proc_data.parse_index += 1;
                            proc_data.chunk_vec.push(String::from(chunk));
                            let sep = if proc_data.new_tag {
                                proc_data.new_tag = false;
                                "\n"
                            } else {
                                ""
                            };
                            proc_data.chunk_group_buf.push_str(&format!(
                                "{}<{} id=\"{}\">{}</{}>",
                                sep, syntax_tag, index, chunk, syntax_tag
                            ));
                        }
                    }
                    Ok(())
                }),
            ],
            ..Settings::default()
        },
        |_c: &[u8]| {},
    );
    rewriter.write(orig_html)?;
    rewriter.end()?;
    {
        let mut proc_data = data.borrow_mut();
        if !proc_data.chunk_group_buf.is_empty() {
            let chunk = std::mem::take(&mut proc_data.chunk_group_buf);
            proc_data.chunk_group_vec.push(chunk);
        }
    }
    Ok(data)
}

pub(crate) fn translate_text(
    llm_interactor: &LlmConnector,
    text: &str,
    chunk_vec: &mut [String],
    syntax_tag: &str,
) -> Result<(), Error> {
    let result = llm_interactor.interact(text)?;
    handle_tagged_result(&result, chunk_vec, syntax_tag)?;
    Ok(())
}

fn serialize_proc_data<P: AsRef<Path>>(path: P, data: &ProcessData) -> Result<(), Error> {
    let bytes = bincode::serialize(data)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

pub(crate) fn translate_html<P: AsRef<Path>>(
    transbot: &TransBot,
    orig_html: &[u8],
    state_file_path: Option<P>,
) -> Result<Vec<u8>, Error> {
    let selector = &transbot.trans_config.html_elem_selector;
    let chunk_size = transbot.trans_config.text_chunk_size;
    let syntax_tag = transbot.trans_config.syntax_tag.as_str();
    let data = if let Some(path) = &state_file_path {
        match std::fs::read(path) {
            Ok(bytes) => {
                let decoded: ProcessData = bincode::deserialize(&bytes)?;
                Rc::new(RefCell::new(decoded))
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {
                html_pass1(selector, orig_html, chunk_size, syntax_tag)?
            }
            Err(e) => {
                return Err(e.into());
            }
        }
    } else {
        html_pass1(selector, orig_html, chunk_size, syntax_tag)?
    };

    if state_file_path.is_some() && transbot.is_interrupted() {
        return Err(TransBot::get_interrupted_error());
    }
    {
        let mut proc_data = data.borrow_mut();
        let start_index = proc_data.trans_index;
        for index in start_index..proc_data.chunk_group_vec.len() {
            let chunk_group = std::mem::take(&mut proc_data.chunk_group_vec[index]);
            match translate_text(
                &transbot.llm_interactor,
                &chunk_group,
                &mut proc_data.chunk_vec,
                syntax_tag,
            ) {
                Ok(_) => {
                    proc_data.trans_index = index + 1;
                }
                Err(e) => {
                    if let Some(path) = &state_file_path
                        && proc_data.trans_index > start_index
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
                if proc_data.trans_index > start_index {
                    let _ = serialize_proc_data(path, &proc_data);
                }
                return Err(TransBot::get_interrupted_error());
            }
        }

        // Save the state file for possible failure before whole job done
        if let Some(path) = &state_file_path
            && proc_data.trans_index > start_index
        {
            let _ = serialize_proc_data(path, &proc_data);
        }

        proc_data.depth = 0;
    }

    html_pass2(data.clone(), selector, orig_html)
}
