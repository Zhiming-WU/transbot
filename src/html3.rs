use anyhow::Error;
use lol_html::{HtmlRewriter, Settings, element, end_tag, text};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::*;

#[derive(Default)]
struct ProcessData {
    start_tag_ended: bool,
    depth: u32,
    index: u32,
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
                        proc_data.start_tag_ended = false;
                    }
                }
                el.on_end_tag(end_tag!(move |end| {
                    let mut proc_data = data2.borrow_mut();
                    if proc_data.depth == 1 {
                        proc_data.index += 1;
                        let index = proc_data.index;
                        let mut elem_buffer =
                            proc_data.trans_map.remove(&index).unwrap_or_default();
                        elem_buffer.push_str(&format!("</{}>", end.name()));
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
            let mut proc_data = data2.borrow_mut();
            if proc_data.depth == 0 || !proc_data.start_tag_ended {
                out.extend_from_slice(c);
                proc_data.start_tag_ended = true;
            }
        },
    );
    rewriter.write(orig_html)?;
    rewriter.end()?;
    Ok(out)
}

pub fn translate_html(
    llm_interactor: &LlmConnector,
    elem_selector: &str,
    orig_html: &[u8],
) -> Result<Vec<u8>, Error> {
    let data = Rc::new(RefCell::new(ProcessData::default()));
    let data1 = data.clone();
    let data2 = data.clone();
    let settings = Settings {
        element_content_handlers: vec![
            element!(elem_selector, move |el| {
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
            }),
            text!("*", move |text_chunk| {
                let mut proc_data = data2.borrow_mut();
                if proc_data.depth > 0 {
                    proc_data.elem_buffer.push_str(text_chunk.as_str());
                }
                Ok(())
            }),
        ],
        ..Settings::default()
    };

    let mut rewriter = HtmlRewriter::new(settings, |_c: &[u8]| {});
    rewriter.write(orig_html)?;

    {
        let data3 = data.clone();
        let mut proc_data = data3.borrow_mut();
        for index in 1..=proc_data.index {
            let text = proc_data.trans_map.get(&index).unwrap();
            let translated = llm_interactor.interact(text)?;
            proc_data.trans_map.insert(index, translated);
        }
        proc_data.index = 0;
    }

    html_pass2(data, elem_selector, orig_html)
}
