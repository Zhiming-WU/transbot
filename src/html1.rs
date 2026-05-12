use anyhow::Error;
use lol_html::{HtmlRewriter, Settings, element, end_tag, text};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::ErrorKind;
use std::rc::Rc;

use crate::*;

#[derive(Default, Serialize, Deserialize)]
struct ProcessData {
    new_tag: bool,
    depth: u32,
    chunk_count: u32,
    pass2_index: u32,
    trans_index: usize,
    text_to_trans: String,
    text_vec: Vec<String>,
    trans_map: HashMap<u32, String>,
}

pub(crate) fn handle_tagged_result(
    text: &str,
    trans_map: &mut HashMap<u32, String>,
) -> Result<(), Error> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut index = 0u32;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if tag_name == SYNTAX_TAG
                    && let Ok(Some(id)) = e.try_get_attribute("id")
                    && let Ok(id) = String::from_utf8_lossy(&id.value).parse::<u32>()
                {
                    index = id;
                }
            }
            Ok(Event::Text(e)) => {
                if index != 0 {
                    trans_map.insert(index, String::from_utf8_lossy(e.as_ref()).into_owned());
                }
            }
            Ok(Event::End(_)) => {
                index = 0;
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
                            proc_data.pass2_index += 1;
                            let index = proc_data.pass2_index;
                            if let Some(text) = proc_data.trans_map.remove(&index) {
                                text_chunk
                                    .replace(&text, lol_html::html_content::ContentType::Text);
                            } else {
                                eprintln!("Can not find trans text for chunk {}: {}", index, chunk);
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

fn html_pass1(elem_selector: &str, orig_html: &[u8]) -> Result<Rc<RefCell<ProcessData>>, Error> {
    let data = Rc::new(RefCell::new(ProcessData::default()));
    let data1 = data.clone();
    let data2 = data.clone();
    let thres = std::env::var("TRANSBOT_TEXT_SIZE_THRES")
        .map(|x| x.parse::<usize>().unwrap_or(0))
        .unwrap_or(0);
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
                        if proc_data.depth == 0 && proc_data.text_to_trans.len() > thres {
                            let mut text = String::new();
                            std::mem::swap(&mut text, &mut proc_data.text_to_trans);
                            proc_data.text_vec.push(text);
                        }
                        Ok(())
                    }))
                }),
                text!("*", move |text_chunk| {
                    let mut proc_data = data2.borrow_mut();
                    if proc_data.depth > 0 {
                        let chunk = text_chunk.as_str();
                        if !chunk.trim().is_empty() {
                            proc_data.chunk_count += 1;
                            let count = proc_data.chunk_count;
                            proc_data.trans_map.insert(count, String::from(chunk));
                            let sep = if proc_data.new_tag {
                                proc_data.new_tag = false;
                                "\n"
                            } else {
                                ""
                            };
                            proc_data.text_to_trans.push_str(&format!(
                                "{}<{} id=\"{}\">{}</{}>",
                                sep, SYNTAX_TAG, count, chunk, SYNTAX_TAG
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
        if !proc_data.text_to_trans.is_empty() {
            let mut text = String::new();
            std::mem::swap(&mut text, &mut proc_data.text_to_trans);
            proc_data.text_vec.push(text);
        }
    }
    Ok(data)
}

fn translate_text(
    llm_interactor: &LlmConnector,
    text: &str,
    trans_map: &mut HashMap<u32, String>,
) -> Result<(), Error> {
    let result = llm_interactor.interact(text)?;
    handle_tagged_result(&result, trans_map)?;
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
    let data = if let Some(path) = &state_file_path {
        match std::fs::read(path) {
            Ok(bytes) => {
                let decoded: ProcessData = bincode::deserialize(&bytes)?;
                Rc::new(RefCell::new(decoded))
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {
                html_pass1(&transbot.trans_config.html_elem_selector, orig_html)?
            }
            Err(e) => {
                return Err(e.into());
            }
        }
    } else {
        html_pass1(&transbot.trans_config.html_elem_selector, orig_html)?
    };

    if state_file_path.is_some() && transbot.is_interrupted() {
        return Err(TransBot::get_interrupted_error());
    }
    {
        let mut proc_data = data.borrow_mut();
        let mut text_vec = Vec::<String>::new();
        std::mem::swap(&mut text_vec, &mut proc_data.text_vec);
        let start_index = proc_data.trans_index;
        for index in start_index..text_vec.len() {
            match translate_text(
                &transbot.llm_interactor,
                &text_vec[index],
                &mut proc_data.trans_map,
            ) {
                Ok(_) => {
                    proc_data.trans_index = index + 1;
                }
                Err(e) => {
                    if let Some(path) = &state_file_path
                        && proc_data.trans_index > start_index
                    {
                        std::mem::swap(&mut text_vec, &mut proc_data.text_vec);
                        let _ = serialize_proc_data(path, &proc_data);
                    }
                    return Err(e);
                }
            }
            if let Some(path) = &state_file_path
                && transbot.is_interrupted()
            {
                if proc_data.trans_index > start_index {
                    if proc_data.trans_index < text_vec.len() {
                        std::mem::swap(&mut text_vec, &mut proc_data.text_vec);
                    }
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

    html_pass2(
        data.clone(),
        &transbot.trans_config.html_elem_selector,
        orig_html,
    )
}
