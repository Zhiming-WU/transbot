use crate::*;
use anyhow::Error;
use rbook::Epub;
use rbook::epub::toc::EpubTocEntryMut;
use std::cell::RefCell;
use std::convert::AsRef;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::rc::Rc;

fn traverse_toc_mut<H>(epub: &mut Epub, mut handler: H)
where
    H: FnMut(Rc<RefCell<EpubTocEntryMut<'_>>>),
{
    let mut toc_mut = epub.toc_mut();

    fn dfs<F>(elem: EpubTocEntryMut<'_>, handler: &mut F)
    where
        F: FnMut(Rc<RefCell<EpubTocEntryMut<'_>>>),
    {
        let rc = Rc::new(RefCell::new(elem));
        handler(rc.clone());
        let mut entry = rc.borrow_mut();
        for sub in entry.iter_mut() {
            dfs(sub, handler);
        }
    }

    for root in toc_mut.iter_mut() {
        dfs(root, &mut handler);
    }
}

fn translate_toc(transbot: &TransBot, epub: &mut Epub) -> Result<(), Error> {
    let mut index = 0usize;
    let mut toc_text = String::new();
    let mut toc_text_vec = Vec::<String>::new();
    let mut chunk_vec = Vec::new();
    let thres = transbot.trans_config.text_chunk_size;
    let syntax_tag = transbot.trans_config.syntax_tag.as_str();

    // pass1
    traverse_toc_mut(epub, |entry| {
        let elem = entry.borrow();
        let view = elem.as_view();
        if !view.label().trim().is_empty() {
            chunk_vec.push(view.label().to_string());
            toc_text.push_str(&format!(
                "<{} id=\"{}\">{}</{}>\n",
                syntax_tag,
                index,
                view.label(),
                syntax_tag,
            ));
            if toc_text.len() > thres {
                let text = std::mem::take(&mut toc_text);
                toc_text_vec.push(text);
            }
            index += 1;
        }
    });
    if !toc_text.is_empty() {
        toc_text_vec.push(toc_text);
    }

    // translating text
    if toc_text_vec.is_empty() {
        return Ok(());
    }
    for text in toc_text_vec {
        check_interrupted(transbot)?;
        let result = transbot.get_llm_interactor().interact(&text)?;
        html1::handle_tagged_result(&result, &mut chunk_vec, syntax_tag)?;
    }

    // pass2
    index = 0;
    traverse_toc_mut(epub, |entry| {
        let mut elem = entry.borrow_mut();
        let label = elem.as_view().label();
        if !label.trim().is_empty() {
            if index < chunk_vec.len() {
                let text = std::mem::take(&mut chunk_vec[index]);
                elem.set_label(text);
            }
            index += 1;
        }
    });

    Ok(())
}

fn translate_metadata(transbot: &TransBot, epub: &mut Epub) -> Result<(), Error> {
    let props = ["dc:title", "dc:creator", "dc:description"];

    // The "dc:description" metadata may be a HTML doc, so wrapping it into XML-like text
    // using SYNTAX_TAG (like the way translating the toc) may cause wrong logic in parsing
    // the result. So translate the metadata in a direct way.
    let mut metadata_mut = epub.metadata_mut();
    for prop in props {
        let entrys = metadata_mut.by_property_mut(prop);
        for mut entry in entrys {
            let view = entry.as_view();
            if !view.value().trim().is_empty() {
                let translated = transbot.get_llm_interactor().interact(view.value())?;
                entry.set_value(translated);
            }
        }
    }

    Ok(())
}

fn load_previous_progress<P: AsRef<Path>>(temp_file: P) -> Result<u32, Error> {
    let mut file = File::open(temp_file)?;
    let mut buffer = [0u8; 4];
    file.read_exact(&mut buffer)?;
    Ok(u32::from_ne_bytes(buffer))
}

fn save_previous_progress<P: AsRef<Path>>(temp_file: P, progress: u32) -> Result<(), Error> {
    let mut file = File::create(temp_file)?;
    let bytes = progress.to_ne_bytes();
    file.write_all(&bytes)?;
    Ok(())
}

fn check_interrupted(transbot: &TransBot) -> Result<(), Error> {
    if transbot.is_interrupted() {
        return Err(TransBot::get_interrupted_error());
    }
    Ok(())
}

pub(crate) fn epub(
    transbot: &TransBot,
    src_path: impl AsRef<Path>,
    dest_path: impl AsRef<Path>,
) -> Result<(), Error> {
    let mut index = 0u32;
    let mut previous_progress = 0u32;
    let temp_path = get_extended_path(dest_path.as_ref(), "temp", true);

    if let Ok(progress) = load_previous_progress(&temp_path) {
        previous_progress = progress;
    }

    let mut epub = match Epub::open(&dest_path) {
        Ok(e) => e,
        Err(_) => {
            previous_progress = 0;
            Epub::open(&src_path)?
        }
    };

    check_interrupted(transbot)?;
    if index >= previous_progress {
        translate_metadata(transbot, &mut epub)?;
        epub.write().compression(9).save(&dest_path)?;
        index += 1;
        save_previous_progress(&temp_path, index)?;
    } else {
        index += 1;
    }

    check_interrupted(transbot)?;
    if index >= previous_progress {
        translate_toc(transbot, &mut epub)?;
        epub.write().compression(9).save(&dest_path)?;
        index += 1;
        save_previous_progress(&temp_path, index)?;
    } else {
        index += 1;
    }

    let spine = epub.spine();
    let mut html_vec = Vec::new();
    for entry in spine.iter() {
        if let Some(res) = entry.resource()
            && res.kind().as_str().contains("htm")
            && let Some(key) = res.key().value()
        {
            html_vec.push(String::from(key));
        }
    }

    let id_vec = epub
        .manifest()
        .iter()
        .filter_map(move |entry| {
            if html_vec.contains(&entry.href().as_str().to_string()) {
                Some(entry.id().to_string())
            } else {
                None
            }
        })
        .collect::<Vec<String>>();

    check_interrupted(transbot)?;
    for id in id_vec {
        if index < previous_progress {
            index += 1;
            continue;
        }
        let mut manifest = epub.manifest_mut();
        let mut entry = manifest.by_id_mut(&id).unwrap();
        let view = entry.as_view();
        let orig_bytes = view.read_bytes()?;
        let chapter_temp_path = get_extended_path(&temp_path, &format!("{}", index), true);
        let trans_bytes =
            transbot.translate_html_resumable(&orig_bytes, Some(chapter_temp_path))?;
        entry.set_content(trans_bytes);
        epub.write().compression(9).save(&dest_path)?;
        index += 1;
        save_previous_progress(&temp_path, index)?;
    }

    let _ = std::fs::remove_file(temp_path);

    Ok(())
}
