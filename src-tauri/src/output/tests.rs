//! `output` 模块的单元测试（从 `output.rs` 拆出）。

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
