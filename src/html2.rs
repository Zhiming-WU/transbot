use anyhow::Error;
use lol_html::{HtmlRewriter, Settings, element, end_tag};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::ErrorKind;
use std::rc::Rc;

use crate::*;

#[derive(Default, Serialize, Deserialize)]
struct ProcessData {
    depth: u32,
    index: u32,
    trans_index: u32,
    elem_buffer: String,
    trans_map: HashMap<u32, String>,
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
            element_content_handlers: vec![element!(elem_selector, move |el| {
                if el.is_self_closing() {
                    return Ok(());
                }
                let data2 = data1.clone();
                {
                    let mut proc_data = data1.borrow_mut();
                    if proc_data.depth > 0 {
                        proc_data.depth += 1;
                    } else {
                        proc_data.depth = 1;
                    }
                }
                el.on_end_tag(end_tag!(move |end| {
                    let mut proc_data = data2.borrow_mut();
                    if proc_data.depth == 1 {
                        proc_data.index += 1;
                        let index = proc_data.index;
                        let elem_buffer = proc_data.trans_map.remove(&index).unwrap_or_default();
                        end.replace(&elem_buffer, lol_html::html_content::ContentType::Html);
                        // Reset state
                        proc_data.depth = 0;
                    } else if proc_data.depth > 1 {
                        proc_data.depth -= 1;
                    }
                    Ok(())
                }))
            })],
            ..Settings::default()
        },
        |c: &[u8]| {
            let proc_data = data2.borrow();
            if proc_data.depth == 0 {
                out.extend_from_slice(c);
            }
        },
    );
    rewriter.write(orig_html)?;
    rewriter.end()?;
    Ok(out)
}

fn html_pass1(elem_selector: &str, orig_html: &[u8]) -> Result<Rc<RefCell<ProcessData>>, Error> {
    let data = Rc::new(RefCell::new(ProcessData::default()));
    let data1 = data.clone();
    let settings = Settings {
        element_content_handlers: vec![element!(elem_selector, move |el| {
            if el.is_self_closing() {
                return Ok(());
            }
            let data2 = data1.clone();
            {
                let mut proc_data = data1.borrow_mut();
                if proc_data.depth > 0 {
                    proc_data.depth += 1;
                } else {
                    proc_data.depth = 1;
                    proc_data.elem_buffer.clear();
                }
            }
            el.on_end_tag(end_tag!(move |_end| {
                let mut proc_data = data2.borrow_mut();
                if proc_data.depth == 1 {
                    proc_data.index += 1;
                    proc_data
                        .elem_buffer
                        .push_str(&format!("</{}>", _end.name()));
                    let index = proc_data.index;
                    let mut elem_buffer = String::new();
                    std::mem::swap(&mut elem_buffer, &mut proc_data.elem_buffer);
                    proc_data.trans_map.insert(index, elem_buffer);
                    // Reset state
                    proc_data.depth = 0;
                } else if proc_data.depth > 1 {
                    proc_data.depth -= 1;
                }
                Ok(())
            }))
        })],
        ..Settings::default()
    };
    let data2 = data.clone();
    let output_sink = move |c: &[u8]| {
        let mut proc_data = data2.borrow_mut();
        if proc_data.depth > 0 {
            proc_data.elem_buffer.push_str(&String::from_utf8_lossy(c));
        }
    };

    let mut rewriter = HtmlRewriter::new(settings, output_sink);
    rewriter.write(orig_html)?;

    Ok(data)
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
    let data: Rc<RefCell<ProcessData>> = if let Some(path) = &state_file_path {
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
        let data3 = data.clone();
        let mut proc_data = data3.borrow_mut();
        let start_index = if proc_data.trans_index == 0 {
            1
        } else {
            proc_data.trans_index
        };
        for index in start_index..=proc_data.index {
            let text = proc_data.trans_map.get(&index).unwrap();
            match transbot.llm_interactor.interact(text) {
                Ok(translated) => {
                    proc_data.trans_map.insert(index, translated);
                    proc_data.trans_index = index + 1;
                }
                Err(e) => {
                    if let Some(path) = &state_file_path
                        && proc_data.trans_index > start_index
                    {
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
        proc_data.index = 0;
    }

    html_pass2(
        data.clone(),
        &transbot.trans_config.html_elem_selector,
        orig_html,
    )
}
