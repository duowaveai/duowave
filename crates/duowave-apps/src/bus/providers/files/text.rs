//! A document's text for «прочитай этот файл» (docs/plans/shared-folders.md, «Новые умения папки»): plain text and
//! Markdown as they are, RTF without its control words, a Word document (`.docx`: a zip of XML) by its paragraphs,
//! a PDF by its text layer — a scan has none, and says so. The text comes by parts, a paragraph never cut where it
//! can be helped, so the voice reads on with «дальше».
use std::io::Read;
use std::path::Path;

/// A part of the text at most, in letters.
pub const PART: usize = 4_000;
/// The text taken of one document at most, in letters (256 KB of Russian is about this).
pub const MOST: usize = 128 * 1024;
/// A document read at most, in bytes.
pub const FILE_MOST: u64 = 32 * 1024 * 1024;

/// The kinds of documents whose text is read.
pub fn readable(name: &str) -> bool {
    let extension = name.rsplit_once('.').map(|(_, extension)| extension.to_ascii_lowercase()).unwrap_or_default();
    matches!(extension.as_str(), "txt" | "md" | "markdown" | "csv" | "log" | "json" | "rtf" | "docx" | "pdf")
}

/// The document's text, as people read it.
pub fn text_of(path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|error| error.to_string())?;
    if meta.len() > FILE_MOST {
        return Err(crate::tr!("документ больше {max} МБ", max = FILE_MOST / 1024 / 1024));
    }
    let name = path.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
    let extension = name.rsplit_once('.').map(|(_, extension)| extension.to_string()).unwrap_or_default();
    let text = match extension.as_str() {
        "docx" => docx(path)?,
        "pdf" => pdf(path)?,
        "rtf" => rtf(&String::from_utf8_lossy(&std::fs::read(path).map_err(|error| error.to_string())?)),
        _ if readable(&name) => String::from_utf8_lossy(&std::fs::read(path).map_err(|error| error.to_string())?)
            .trim_start_matches('\u{feff}')
            .to_string(),
        _ => return Err(crate::tr!("это не документ: текст читается у txt, md, rtf, docx и pdf")),
    };
    let text = tidy(&text);
    if text.trim().is_empty() {
        return Err(if extension == "pdf" {
            crate::tr!("в этом PDF нет текста — похоже, это скан")
        } else {
            crate::tr!("документ пустой")
        });
    }
    Ok(text.chars().take(MOST).collect())
}

/// Paragraphs kept, runs of spaces and empty lines made one.
fn tidy(text: &str) -> String {
    let mut out = Vec::new();
    let mut empty = 0;
    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            empty += 1;
            if empty == 1 && !out.is_empty() {
                out.push(String::new());
            }
            continue;
        }
        empty = 0;
        out.push(line);
    }
    out.join("\n").trim().to_string()
}

/// A Word document: the text runs of `word/document.xml`, a paragraph a line.
fn docx(path: &Path) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|_| crate::tr!("файл Word не читается"))?;
    let mut document = archive.by_name("word/document.xml").map_err(|_| crate::tr!("в файле Word нет текста"))?;
    let mut xml = String::new();
    document.by_ref().take(FILE_MOST).read_to_string(&mut xml).map_err(|_| crate::tr!("файл Word не читается"))?;
    Ok(word_text(&xml))
}

/// The text of WordprocessingML: `<w:t>` runs, `<w:tab/>` and `<w:br/>`, a paragraph `</w:p>` a line.
fn word_text(xml: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    while let Some(start) = rest.find('<') {
        let Some(end) = rest[start..].find('>') else { break };
        let tag = &rest[start + 1..start + end];
        let after = &rest[start + end + 1..];
        let name = tag.trim_start_matches('/').split([' ', '/']).next().unwrap_or_default();
        match name {
            "w:t" if !tag.starts_with('/') && !tag.ends_with('/') => {
                let close = after.find("</w:t>").unwrap_or(after.len());
                out.push_str(&unescape(&after[..close]));
                rest = &after[close..];
                continue;
            }
            "w:tab" => out.push('\t'),
            "w:br" | "w:cr" => out.push('\n'),
            "w:p" if tag.starts_with('/') => out.push('\n'),
            _ => {}
        }
        rest = after;
    }
    out
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// A PDF's text layer, page after page.
fn pdf(path: &Path) -> Result<String, String> {
    let document = lopdf::Document::load(path).map_err(|_| crate::tr!("PDF не читается"))?;
    if document.is_encrypted() {
        return Err(crate::tr!("PDF под паролем"));
    }
    let pages: Vec<u32> = document.get_pages().keys().copied().collect();
    let mut out = String::new();
    for page in pages {
        if let Ok(text) = document.extract_text(&[page]) {
            out.push_str(&text);
            out.push('\n');
        }
        if out.chars().count() > MOST {
            break;
        }
    }
    Ok(out)
}

/// RTF without its groups' control words and the hidden ones (fonts, colours, styles, pictures).
fn rtf(source: &str) -> String {
    const HIDDEN: &[&str] = &["fonttbl", "colortbl", "stylesheet", "info", "pict", "header", "footer", "*"];
    let mut out = String::new();
    let mut depth_hidden: Vec<bool> = vec![false];
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        let hidden = *depth_hidden.last().unwrap_or(&false);
        match c {
            '{' => depth_hidden.push(hidden),
            '}' => {
                depth_hidden.pop();
                if depth_hidden.is_empty() {
                    depth_hidden.push(false);
                }
            }
            '\\' => {
                let mut word = String::new();
                while let Some(&next) = chars.peek() {
                    if next.is_ascii_alphabetic() {
                        word.push(next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if word.is_empty() {
                    match chars.next() {
                        Some('\'') => {
                            let hex: String = chars.by_ref().take(2).collect();
                            if let (false, Ok(byte)) = (hidden, u8::from_str_radix(&hex, 16)) {
                                // Windows-1251, the usual code page of a Russian RTF.
                                let bytes = [byte];
                                let (decoded, _, _) = encoding_rs::WINDOWS_1251.decode(&bytes);
                                out.push_str(&decoded);
                            }
                        }
                        Some('*') => {
                            if let Some(last) = depth_hidden.last_mut() {
                                *last = true;
                            }
                        }
                        Some(other) if !hidden && "\\{}".contains(other) => out.push(other),
                        _ => {}
                    }
                    continue;
                }
                let mut number = String::new();
                while let Some(&next) = chars.peek() {
                    if next == '-' || next.is_ascii_digit() {
                        number.push(next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if chars.peek() == Some(&' ') {
                    chars.next();
                }
                if HIDDEN.contains(&word.as_str()) {
                    if let Some(last) = depth_hidden.last_mut() {
                        *last = true;
                    }
                    continue;
                }
                if hidden {
                    continue;
                }
                match word.as_str() {
                    "par" | "line" => out.push('\n'),
                    "tab" => out.push('\t'),
                    "u" => {
                        if let Some(letter) = number.parse::<i32>().ok().and_then(|code| {
                            char::from_u32(if code < 0 { (code + 65_536) as u32 } else { code as u32 })
                        }) {
                            out.push(letter);
                            // The fallback letter after \uN.
                            if chars.peek().is_some_and(|next| *next != '\\' && *next != '{' && *next != '}') {
                                chars.next();
                            }
                        }
                    }
                    _ => {}
                }
            }
            '\r' | '\n' => {}
            other if !hidden => out.push(other),
            _ => {}
        }
    }
    out
}

/// The `part`-th part of the text (from 0) and how many there are: about `PART` letters each, cut at a paragraph's
/// end, else at a sentence's, else at a space.
pub fn part(text: &str, part: usize) -> (String, usize) {
    let mut parts = Vec::new();
    let mut rest: &str = text;
    while !rest.is_empty() {
        let limit = rest.char_indices().nth(PART).map(|(at, _)| at).unwrap_or(rest.len());
        if limit == rest.len() {
            parts.push(rest);
            break;
        }
        let window = &rest[..limit];
        let cut = window
            .rfind("\n")
            .filter(|at| *at > limit / 2)
            .or_else(|| window.rfind(". ").filter(|at| *at > limit / 2).map(|at| at + 1))
            .or_else(|| window.rfind(' '))
            .unwrap_or(limit);
        let cut = cut.max(1);
        parts.push(rest[..cut].trim_end());
        rest = rest[cut..].trim_start();
    }
    let count = parts.len().max(1);
    (parts.get(part).map(|part| part.to_string()).unwrap_or_default(), count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_document_reads_by_its_paragraphs() {
        let xml = r#"<w:document><w:body><w:p><w:r><w:t>Борщ</w:t></w:r></w:p><w:p><w:r><w:t xml:space="preserve">Свёкла, </w:t></w:r><w:r><w:t>капуста &amp; лук</w:t></w:r></w:p></w:body></w:document>"#;
        assert_eq!(tidy(&word_text(xml)), "Борщ\nСвёкла, капуста & лук");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("рецепт.docx");
        {
            let file = std::fs::File::create(&path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file("word/document.xml", zip::write::SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut zip, xml.as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        assert_eq!(text_of(&path).unwrap(), "Борщ\nСвёкла, капуста & лук");
    }

    #[test]
    fn rtf_loses_its_control_words_and_keeps_the_letters() {
        let source = r"{\rtf1\ansi\ansicpg1251{\fonttbl{\f0 Arial;}}\f0\fs24 \'c1\'ee\'f0\'f9\par \u1089?\u1074?\u1105?\u1082?\u1083?\u1072?}";
        assert_eq!(tidy(&rtf(source)), "Борщ\nсвёкла");
    }

    #[test]
    fn plain_text_and_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.md"), "\u{feff}# Блины\n\n\n\nДва  яйца").unwrap();
        assert_eq!(text_of(&dir.path().join("a.md")).unwrap(), "# Блины\n\nДва яйца");
        std::fs::write(dir.path().join("a.jpg"), "x").unwrap();
        assert!(text_of(&dir.path().join("a.jpg")).is_err());
        assert!(readable("Отчёт.PDF") && !readable("фото.jpg"));
    }

    #[test]
    fn a_pdf_gives_its_text_layer() {
        use lopdf::content::{Content, Operation};
        use lopdf::{dictionary, Object, Stream};
        let mut doc = lopdf::Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Courier",
        });
        let resources_id = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font_id } });
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 24.into()]),
                Operation::new("Td", vec![100.into(), 600.into()]),
                Operation::new("Tj", vec![Object::string_literal("Hello from the PDF")]),
                Operation::new("ET", vec![]),
            ],
        };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content_id,
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1,
                "Resources" => resources_id, "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            }),
        );
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog_id);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("отчёт.pdf");
        doc.save(&path).unwrap();
        assert!(text_of(&path).unwrap().contains("Hello from the PDF"));
    }

    #[test]
    fn parts_are_cut_at_paragraphs() {
        let paragraph = "Слово ".repeat(500);
        let text = format!("{}\n{}\n{}", paragraph.trim(), paragraph.trim(), paragraph.trim());
        let (first, count) = part(&text, 0);
        assert!(count >= 2, "{count}");
        assert!(first.chars().count() <= PART);
        assert!(first.ends_with("Слово"));
        let (last, _) = part(&text, count - 1);
        assert!(!last.is_empty());
        assert_eq!(part(&text, count).0, "");
        assert_eq!(part("коротко", 0), ("коротко".to_string(), 1));
    }
}
