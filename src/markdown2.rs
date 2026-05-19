use anyhow::Error;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use pulldown_cmark_to_cmark::cmark_with_options;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::io::ErrorKind;
use std::rc::Rc;

use crate::*;

#[derive(Default, Serialize, Deserialize)]
struct ProcessData {
    to_accept_text: bool,
    in_pass2: bool,
    is_needed_code_block: bool,
    depth: u32,
    parse_index: usize,
    trans_index: usize,
    text_buf: String,
    text_vec: Vec<String>,
    pass2_out: String,
}

struct ProcessDataWrapper(Rc<RefCell<ProcessData>>);

impl std::fmt::Write for ProcessDataWrapper {
    fn write_str(&mut self, s: &str) -> Result<(), std::fmt::Error> {
        // println!("Recv: {}", s);
        let mut proc_data = self.0.borrow_mut();
        if proc_data.to_accept_text {
            proc_data.text_buf.push_str(s);
        } else if proc_data.in_pass2 {
            proc_data.pass2_out.push_str(s);
        }
        Ok(())
    }
}

struct EventBorrowInfo {
    is_first: bool,
    borrow_cnt: u8,
}

struct EventWrapper<'a> {
    borrow_info: RefCell<EventBorrowInfo>,
    event: Event<'a>,
    data: Rc<RefCell<ProcessData>>,
}

impl<'a> EventWrapper<'a> {
    fn new(is_first: bool, event: Event<'a>, data: Rc<RefCell<ProcessData>>) -> Self {
        Self {
            borrow_info: RefCell::new(EventBorrowInfo {
                is_first,
                borrow_cnt: 0,
            }),
            event,
            data,
        }
    }
}

impl<'a> std::borrow::Borrow<Event<'a>> for EventWrapper<'a> {
    fn borrow(&self) -> &Event<'a> {
        let mut info = self.borrow_info.borrow_mut();
        if (info.is_first && info.borrow_cnt == 0) || (!info.is_first && info.borrow_cnt == 1) {
            let mut proc_data = self.data.borrow_mut();
            // println!(
            //     "DebugEvent: {:?}\n[{}], [{}], ",
            //     &self.event, proc_data.to_accept_text, &proc_data.text_buf,
            // );
            match &self.event {
                Event::Start(tag) => {
                    if proc_data.depth == 0 {
                        proc_data.to_accept_text = false;
                        if !proc_data.text_buf.trim().is_empty() {
                            let text = std::mem::take(&mut proc_data.text_buf);
                            // println!("ParsedText: {}", &text);
                            if !proc_data.in_pass2 {
                                proc_data.text_vec.push(text);
                            } else {
                                let index = proc_data.parse_index;
                                if index < proc_data.text_vec.len() {
                                    let t = std::mem::take(&mut proc_data.text_vec[index]);
                                    // println!("ReplaceText: {}", &t);
                                    proc_data.pass2_out.push_str(&t);
                                }
                                proc_data.parse_index += 1;
                            }
                        }
                        proc_data.text_buf.clear();
                    }
                    match tag {
                        Tag::Paragraph
                        | Tag::Heading { .. }
                        | Tag::List(_)
                        | Tag::MetadataBlock(_)
                        | Tag::Table(_)
                        | Tag::HtmlBlock
                        | Tag::FootnoteDefinition(_) => {
                            if proc_data.depth == 0 {
                                proc_data.to_accept_text = true;
                            }
                            proc_data.depth += 1;
                        }
                        Tag::CodeBlock(CodeBlockKind::Fenced(kind)) => {
                            if kind.starts_with("admonish") {
                                proc_data.depth += 1;
                                proc_data.to_accept_text = true;
                                proc_data.is_needed_code_block = true;
                            } else {
                                proc_data.is_needed_code_block = false;
                            }
                        }
                        _ => {}
                    }
                }
                Event::End(
                    TagEnd::Paragraph
                    | TagEnd::Heading { .. }
                    | TagEnd::List(_)
                    | TagEnd::MetadataBlock(_)
                    | TagEnd::Table
                    | TagEnd::HtmlBlock
                    | TagEnd::FootnoteDefinition,
                ) => {
                    if proc_data.depth > 0 {
                        proc_data.depth -= 1;
                    }
                }
                Event::End(TagEnd::CodeBlock) => {
                    if proc_data.is_needed_code_block {
                        if proc_data.depth > 0 {
                            proc_data.depth -= 1;
                        }
                        proc_data.is_needed_code_block = false;
                    }
                }
                _ => {}
            }
        }
        info.borrow_cnt += 1;
        &self.event
    }
}

fn markdown_pass2(data: Rc<RefCell<ProcessData>>, orig_markdown: &str) -> Result<Vec<u8>, Error> {
    {
        let mut proc_data = data.borrow_mut();
        proc_data.depth = 0;
        proc_data.parse_index = 0;
        proc_data.text_buf.clear();
        proc_data.to_accept_text = false;
        proc_data.in_pass2 = true;
    }
    let parser = get_cmark_parser(orig_markdown);
    let mut is_first = true;
    let events = parser.map(|event| {
        let event = EventWrapper::new(is_first, event, data.clone());
        is_first = false;
        event
    });
    let mut wrapper = ProcessDataWrapper(data.clone());
    cmark_with_options(events, &mut wrapper, get_to_cmark_options())?;
    {
        let mut proc_data = data.borrow_mut();
        if !proc_data.text_buf.trim().is_empty() {
            let index = proc_data.parse_index;
            if index < proc_data.text_vec.len() {
                let t = std::mem::take(&mut proc_data.text_vec[index]);
                proc_data.pass2_out.push_str(&t);
            }
            proc_data.parse_index += 1;
        }
        let out = std::mem::take(&mut proc_data.pass2_out);
        Ok(out.into_bytes())
    }
}

fn get_cmark_parser(md_text: &str) -> Parser<'_> {
    Parser::new_ext(md_text, Options::all())
}

pub(crate) fn get_to_cmark_options<'a>() -> pulldown_cmark_to_cmark::Options<'a> {
    pulldown_cmark_to_cmark::Options::<'_> {
        increment_ordered_list_bullets: true,
        list_token: '-',
        ..Default::default()
    }
}

fn markdown_pass1(orig_markdown: &str) -> Result<Rc<RefCell<ProcessData>>, Error> {
    let data = Rc::new(RefCell::new(ProcessData::default()));
    let parser = get_cmark_parser(orig_markdown);
    let mut is_first = true;
    let events = parser.map(|event| {
        let event = EventWrapper::new(is_first, event, data.clone());
        is_first = false;
        event
    });
    let mut wrapper = ProcessDataWrapper(data.clone());
    cmark_with_options(events, &mut wrapper, get_to_cmark_options())?;
    {
        let mut proc_data = data.borrow_mut();
        if !proc_data.text_buf.trim().is_empty() {
            let text = std::mem::take(&mut proc_data.text_buf);
            proc_data.text_vec.push(text);
        }
    }
    Ok(data)
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
    let proc_data = if let Some(path) = &state_file_path {
        match std::fs::read(path) {
            Ok(bytes) => {
                let decoded: ProcessData = bincode::deserialize(&bytes)?;
                Rc::new(RefCell::new(decoded))
            }
            Err(e) if e.kind() == ErrorKind::NotFound => markdown_pass1(&orig_text)?,
            Err(e) => {
                return Err(e.into());
            }
        }
    } else {
        markdown_pass1(&orig_text)?
    };

    if state_file_path.is_some() && transbot.is_interrupted() {
        return Err(TransBot::get_interrupted_error());
    }
    {
        let mut proc_data = proc_data.borrow_mut();
        let start_index = proc_data.trans_index;
        for index in start_index..proc_data.text_vec.len() {
            let text = &proc_data.text_vec[index];
            match transbot.llm_interactor.interact(text) {
                Ok(mut translated) => {
                    restore_triming_newlines(&mut translated, text.as_str());
                    proc_data.text_vec[index] = translated;
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
    }

    markdown_pass2(proc_data, &orig_text)
}
