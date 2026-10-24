//! 保存转写结果：文件名清洗 + 落盘（同名自动加序号，不覆盖）。

use std::fs;
use std::path::PathBuf;

use serde::Serialize;

use crate::code_detail;
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Saved {
    pub path: String,
    pub name: String,
}

pub fn sanitize_name(input: &str) -> Option<String> {
    let cleaned: String = input
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\n' | '\r' | '\t' => ' ',
            _ => ch,
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.chars().take(80).collect())
    }
}

pub fn write_text_file(
    dir: &std::path::Path,
    requested: &str,
    text: &str,
) -> Result<(PathBuf, String), String> {
    let base = sanitize_name(requested).unwrap_or_else(|| "transcript".to_string());
    let name = if base.to_lowercase().ends_with(".txt") {
        base
    } else {
        format!("{base}.txt")
    };
    // 按字节切后缀（.txt 是 ASCII，切点一定在字符边界上）。
    // 不能 trim_end_matches(".txt")：大写 .TXT 会切不掉，序号文件名变成 "x.TXT (2).txt"。
    let stem = name[..name.len() - 4].to_string();
    let mut path = dir.join(&name);
    let mut index = 2;
    while path.exists() {
        path = dir.join(format!("{stem} ({index}).txt"));
        index += 1;
    }
    fs::write(&path, text).map_err(|e| code_detail("error.writeFailed", e))?;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.clone());
    Ok((path, file_name))
}


#[cfg(test)]
mod tests {
    use super::*;

        #[test]
        fn sanitizes_dangerous_file_names() {
            assert!(sanitize_name("../../etc/passwd").is_some());
            assert!(sanitize_name("/tmp/x").unwrap().contains("tmp"));
            assert_eq!(sanitize_name("   "), None);
            assert!(sanitize_name("会议记录: 周一").is_some());
        }

        #[test]
        fn writes_unique_files_without_overwriting() {
            let dir = std::env::temp_dir().join(format!("vn-save-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();

            let (first, name1) = write_text_file(&dir, "会议记录", "一").unwrap();
            let (second, name2) = write_text_file(&dir, "会议记录", "二").unwrap();

            assert_eq!(name1, "会议记录.txt");
            assert_eq!(name2, "会议记录 (2).txt");
            assert_eq!(fs::read_to_string(&first).unwrap(), "一");
            assert_eq!(fs::read_to_string(&second).unwrap(), "二");
            let _ = fs::remove_dir_all(&dir);
        }

        /// 大写 .TXT 也要当成已有扩展名：否则序号文件会变成 "会议.TXT (2).txt"。
        #[test]
        fn keeps_uppercase_txt_extension() {
            let dir = std::env::temp_dir().join(format!("vn-save-upper-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();

            let (first, _) = write_text_file(&dir, "会议.TXT", "一").unwrap();
            let (_, name2) = write_text_file(&dir, "会议.TXT", "二").unwrap();

            assert_eq!(first.file_name().unwrap(), "会议.TXT");
            assert_eq!(name2, "会议 (2).txt");
            let _ = fs::remove_dir_all(&dir);
        }
}
