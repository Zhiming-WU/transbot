use anyhow::Error;
use serde::{Deserialize, Serialize};
use std::io::ErrorKind;
use text_splitter::TextSplitter;

use crate::*;

#[derive(Default, Serialize, Deserialize)]
struct ProcessData {
    trans_index: usize,
    text_vec: Vec<String>,
}

fn text_pass1(chunk_size: usize, orig_text: &[u8]) -> ProcessData {
    let text = String::from_utf8_lossy(orig_text);
    let splitter = TextSplitter::new(chunk_size);
    let text_vec: Vec<String> = splitter
        .chunks(&text)
        .map(|chunk| chunk.to_string())
        .collect();
    ProcessData {
        trans_index: 0,
        text_vec,
    }
}

fn text_pass2(data: ProcessData) -> Vec<u8> {
    let mut out = String::new();
    for chunk in data.text_vec {
        out.push_str(&chunk);
        out.push_str("\n\n");
    }
    out.into_bytes()
}

fn serialize_proc_data<P: AsRef<Path>>(path: P, data: &ProcessData) -> Result<(), Error> {
    let bytes = bincode::serialize(data)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

pub(crate) fn translate_text<P: AsRef<Path>>(
    transbot: &TransBot,
    orig_text: &[u8],
    state_file_path: Option<P>,
) -> Result<Vec<u8>, Error> {
    let mut proc_data = if let Some(path) = &state_file_path {
        match std::fs::read(path) {
            Ok(bytes) => {
                let decoded: ProcessData = bincode::deserialize(&bytes)?;
                decoded
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {
                text_pass1(transbot.trans_config.text_chunk_size, orig_text)
            }
            Err(e) => {
                return Err(e.into());
            }
        }
    } else {
        text_pass1(transbot.trans_config.text_chunk_size, orig_text)
    };

    if state_file_path.is_some() && transbot.is_interrupted() {
        return Err(TransBot::get_interrupted_error());
    }
    {
        let start_index = proc_data.trans_index;
        for index in start_index..proc_data.text_vec.len() {
            match transbot.llm_interactor.interact(&proc_data.text_vec[index]) {
                Ok(text) => {
                    proc_data.text_vec[index] = text;
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

    Ok(text_pass2(proc_data))
}
