//! `log` 模块的单元测试（从 `log.rs` 拆出）。

use super::*;

/// 临时日志目录（每次调用一个新名字，测试之间不互相踩）。
fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "voice-notes-log-test-{}-{}-{tag}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

/// 目录里的历史日志文件名（按名字排序）。
fn rotated_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("读目录")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(ROTATED_PREFIX))
        .collect();
    names.sort();
    names
}

/// 日志文件真的写出来了、超限会轮转 —— 不依赖 AppHandle，临时目录里跑。
#[test]
fn writes_lines_and_rotates() {
    let dir = temp_dir("basic");
    let file = dir.join(FILE_NAME);

    write_line(&dir, "2026-01-01 00:00:00.000 [INFO] 第一行");
    write_line(&dir, "2026-01-01 00:00:01.000 [INFO] 第二行");
    let body = fs::read_to_string(&file).expect("读日志");
    assert!(body.contains("第一行"), "第一行没写进去：{body}");
    assert!(body.contains("第二行"), "第二行没写进去：{body}");

    // 写爆上限 → 轮转：旧内容进历史文件，新内容进当前文件。
    fs::write(&file, vec![b'x'; MAX_BYTES as usize + 16]).expect("写大文件");
    write_line(&dir, "2026-01-01 00:00:02.000 [INFO] 轮转之后");

    let rotated = rotated_names(&dir);
    assert_eq!(
        rotated.len(),
        1,
        "轮转后应该正好有一个历史文件：{rotated:?}"
    );
    assert!(
        rotated[0].starts_with("voice-notes.log.20"),
        "历史文件名应该带时间戳：{}",
        rotated[0]
    );

    let current = fs::read_to_string(&file).expect("读当前日志");
    assert!(
        current.contains("轮转之后"),
        "轮转后新行没写进去：{current}"
    );
    assert!(!current.contains('x'), "轮转后当前文件还是旧内容");

    // 旧内容进了历史文件。
    let archived = fs::read_to_string(dir.join(&rotated[0])).expect("读历史日志");
    assert!(archived.contains('x'), "旧内容没进历史文件：{archived:?}");

    let _ = fs::remove_dir_all(&dir);
}

/// 旧格式的 `voice-notes.log.1` 不该再产生 —— 它是升级前的产物。
#[test]
fn no_longer_creates_the_legacy_dot_one_file() {
    let dir = temp_dir("legacy");
    let file = dir.join(FILE_NAME);

    fs::write(&file, vec![b'x'; MAX_BYTES as usize + 16]).expect("写大文件");
    write_line(&dir, "2026-01-01 00:00:02.000 [INFO] 轮转之后");

    assert!(
        !dir.join("voice-notes.log.1").exists(),
        "不该再生成旧格式的 .log.1"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// 判定：超限就轮转。
#[test]
fn rotates_when_over_the_size_limit() {
    let now = Local::now();
    assert!(
        should_rotate(MAX_BYTES, Some(SystemTime::now()), now),
        "超限就该轮转"
    );
}

/// 判定：没超限但跨天就轮转。
#[test]
fn rotates_when_the_day_changed() {
    let yesterday = Local::now() - chrono::Duration::days(1);
    let modified = SystemTime::from(yesterday);
    assert!(
        should_rotate(10, Some(modified), Local::now()),
        "跨天就该轮转"
    );
}

/// 判定：没超限也没跨天 → 不轮转。
#[test]
fn does_not_rotate_within_the_same_day() {
    let now = Local::now();
    assert!(
        !should_rotate(10, Some(SystemTime::now()), now),
        "同一天不该轮转"
    );
}

/// 判定：拿不到 mtime → 只看超限，不误轮转。
#[test]
fn missing_mtime_only_triggers_on_size() {
    let now = Local::now();
    assert!(!should_rotate(10, None, now), "拿不到 mtime 不该误轮转");
    assert!(
        should_rotate(MAX_BYTES, None, now),
        "拿不到 mtime 时超限仍要轮转"
    );
}

/// 历史文件名：带时间戳，排序键能取回来。
#[test]
fn rotated_names_carry_a_parsable_timestamp() {
    let name = rotated_name(Local::now());
    assert!(name.starts_with(ROTATED_PREFIX), "名字前缀不对：{name}");
    let key = rotated_key(&name).expect("时间戳应该能解析");
    let expected = Local::now();
    assert_eq!(
        key.date_naive(),
        expected.date_naive(),
        "解析出的日期不对：{key}"
    );
}

/// 旧格式的 `.log.1` 解析不出时间戳 → 排序键为空 → 当作最旧。
#[test]
fn the_legacy_file_sorts_as_the_oldest() {
    assert!(
        rotated_key("voice-notes.log.1").is_none(),
        "旧格式不该解析出时间戳"
    );
    assert!(
        rotated_key("voice-notes.log.2026-01-01-00-00-00").is_some(),
        "新格式应该能解析出时间戳"
    );
    assert!(
        rotated_key("voice-notes.log.2026-01-01-00-00-00-1").is_some(),
        "带计数器的名字也应该能解析出时间戳"
    );
    assert!(
        rotated_key("something-else.log").is_none(),
        "别的文件不该被当成历史文件"
    );
}

/// 保留份数：造 7 个历史文件 → 轮转后只剩 5 个，最旧的被删。
#[test]
fn keeps_only_the_five_newest_archives() {
    let dir = temp_dir("keep");
    let file = dir.join(FILE_NAME);

    for day in 1..=7 {
        fs::write(
            dir.join(format!("voice-notes.log.2026-01-0{day}-00-00-00")),
            format!("第 {day} 份"),
        )
        .expect("造历史文件");
    }
    assert_eq!(rotated_names(&dir).len(), 7, "前置条件：7 个历史文件");

    fs::write(&file, vec![b'x'; MAX_BYTES as usize + 16]).expect("写大文件");
    write_line(&dir, "2026-01-08 00:00:00.000 [INFO] 轮转之后");

    let names = rotated_names(&dir);
    assert_eq!(
        names.len(),
        KEEP_ROTATED,
        "历史文件应剩 {KEEP_ROTATED} 份：{names:?}"
    );
    assert!(
        !names.iter().any(|n| n.contains("2026-01-01")),
        "最旧的一份应该被删掉：{names:?}"
    );
    assert!(
        names.iter().any(|n| n.contains("2026-01-07")),
        "较新的几份应该留着：{names:?}"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// 旧格式的 `.log.1` 会被优先淘汰。
#[test]
fn the_legacy_file_is_pruned_first() {
    let dir = temp_dir("prune-legacy");
    let file = dir.join(FILE_NAME);

    fs::write(dir.join("voice-notes.log.1"), "旧格式").expect("造旧格式文件");
    for day in 1..=KEEP_ROTATED {
        fs::write(
            dir.join(format!("voice-notes.log.2026-01-0{day}-00-00-00")),
            format!("第 {day} 份"),
        )
        .expect("造历史文件");
    }

    fs::write(&file, vec![b'x'; MAX_BYTES as usize + 16]).expect("写大文件");
    write_line(&dir, "2026-01-08 00:00:00.000 [INFO] 轮转之后");

    let names = rotated_names(&dir);
    assert_eq!(
        names.len(),
        KEEP_ROTATED,
        "历史文件应剩 {KEEP_ROTATED} 份：{names:?}"
    );
    assert!(
        !names.iter().any(|n| n == "voice-notes.log.1"),
        "旧格式应该被淘汰：{names:?}"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// 同名轮转文件不覆盖既有内容 —— 撞名时加计数器。
#[test]
fn a_colliding_name_does_not_overwrite() {
    let dir = temp_dir("collide");
    let file = dir.join(FILE_NAME);

    let stamp = Local::now();
    let base = dir.join(rotated_name(stamp));
    fs::write(&base, "先到先得").expect("造同名文件");

    fs::write(&file, vec![b'x'; MAX_BYTES as usize + 16]).expect("写大文件");
    write_line(&dir, "2026-01-01 00:00:02.000 [INFO] 轮转之后");

    let kept = fs::read_to_string(&base).expect("读原有文件");
    assert_eq!(kept, "先到先得", "既有文件被覆盖了：{kept}");

    let names = rotated_names(&dir);
    assert_eq!(names.len(), 2, "应该多出一个带计数器的历史文件：{names:?}");
    assert!(
        names.iter().any(|n| n.ends_with("-1")),
        "撞名应该生成带 -1 后缀的文件：{names:?}"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// 跨天轮转：文件是昨天写的，今天写一行就该轮转。
#[test]
fn rotates_when_the_file_is_from_yesterday() {
    let dir = temp_dir("yesterday");
    let file = dir.join(FILE_NAME);

    // 写一个昨天的时间戳进去（靠文件 mtime 判定，所以要改 mtime）。
    fs::write(&file, "2026-01-01 00:00:00.000 [INFO] 昨天写的").expect("写文件");
    let yesterday = Local::now() - chrono::Duration::days(1);
    let handle = fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .expect("打开日志");
    handle
        .set_modified(SystemTime::from(yesterday))
        .expect("改 mtime");
    drop(handle);

    write_line(&dir, "2026-01-02 00:00:00.000 [INFO] 今天写的");

    let names = rotated_names(&dir);
    assert_eq!(names.len(), 1, "跨天应该轮转出一个历史文件：{names:?}");
    let archived = fs::read_to_string(dir.join(&names[0])).expect("读历史日志");
    assert!(
        archived.contains("昨天写的"),
        "旧内容没进历史文件：{archived}"
    );
    let current = fs::read_to_string(&file).expect("读当前日志");
    assert!(current.contains("今天写的"), "新行没写进去：{current}");
    assert!(
        !current.contains("昨天写的"),
        "当前文件不该有旧内容：{current}"
    );

    let _ = fs::remove_dir_all(&dir);
}
