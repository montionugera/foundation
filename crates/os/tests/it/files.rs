//! `os::files` on the real disk, through `env::files`.

use std::cell::Cell;
use std::future::poll_fn;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::task::Poll;

use block::{Block, Pool};
use env::files::{Absolute, Error, File, Files, Mode, Operation};

use crate::disk::{KIB, Scratch, files, opened};

/// Runs `body` with the files of a scratch directory of its own and the path of
/// their data directory.
fn run<F: Future<Output = ()>>(body: impl FnOnce(Files, PathBuf) -> F) {
    let scratch = Scratch::new();
    let (files, thread) = files(&scratch.0, "files");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(body(files, scratch.0.join("data")));
    thread.join().unwrap();
}

/// `path` as the absolute path that `open_absolute` takes.
fn absolute(path: &Path) -> Absolute {
    Absolute::new(path.to_str().unwrap()).unwrap()
}

fn pool() -> Pool {
    let config = block::Config::new(1 << 20).expect("the budget fits");
    let memory = block::Heap::new(config.reservation());
    Pool::new(config, memory)
}

fn block(pool: &Pool, bytes: &[u8]) -> Block {
    let mut unique = pool.alloc(bytes.len()).unwrap();
    unique.copy_from_slice(bytes);
    unique.freeze()
}

/// The `len` bytes of `file` at `offset`.
async fn read(file: &File, pool: &Pool, offset: u64, len: usize) -> Vec<u8> {
    let into = pool.alloc(len).unwrap();
    file.read_at(offset, into).await.unwrap().to_vec()
}

async fn create(files: &Files, path: &str, len: u64) -> File {
    let mode = Mode::Create { len };
    files.open(Path::new(path), mode).await.unwrap()
}

fn io(path: &str, operation: Operation, code: i32) -> Error {
    Error::Io {
        path: path.into(),
        operation,
        code,
    }
}

#[test]
fn a_read_sees_the_parts_of_a_write_back_to_back() {
    run(|files, _| async move {
        let pool = pool();
        let file = create(&files, "a", 4 * KIB).await;
        let parts = [block(&pool, b"abc"), block(&pool, b"defgh")];
        file.write_at(1_000, &parts).await.unwrap();
        assert_eq!(read(&file, &pool, 999, 10).await, b"\0abcdefgh\0");
    });
}

#[test]
fn a_write_of_more_parts_than_one_call_takes_keeps_their_order() {
    run(|files, _| async move {
        let pool = pool();
        let file = create(&files, "a", 8 * KIB).await;
        let bytes: Vec<[u8; 4]> = (0..1_500_u32).map(u32::to_le_bytes).collect();
        let parts: Vec<Block> = bytes.iter().map(|bytes| block(&pool, bytes)).collect();
        file.write_at(0, &parts).await.unwrap();
        assert_eq!(read(&file, &pool, 0, 6_000).await, bytes.concat());
    });
}

#[test]
fn a_dropped_write_ends_before_a_later_read() {
    run(|files, _| async move {
        let pool = pool();
        let file = create(&files, "a", 4 * KIB).await;
        {
            let parts = [block(&pool, b"kept")];
            let mut write = pin!(file.write_at(8, &parts));
            poll_fn(|context| {
                if let Poll::Ready(result) = write.as_mut().poll(context) {
                    result.unwrap();
                }
                Poll::Ready(())
            })
            .await;
        }
        assert_eq!(read(&file, &pool, 8, 4).await, b"kept");
    });
}

#[test]
fn create_makes_a_zeroed_file_of_its_length_in_the_data_directory() {
    run(|files, data| async move {
        let pool = pool();
        files.create_dir(Path::new("ring")).await.unwrap();
        let file = create(&files, "ring/0", 12 * KIB).await;
        assert_eq!(file.len(), 12 * KIB);
        assert_eq!(read(&file, &pool, 0, 12 * 1_024).await, vec![0; 12 * 1_024]);
        let found = std::fs::metadata(data.join("ring/0")).unwrap();
        assert_eq!(found.len(), 12 * KIB);
    });
}

#[test]
fn create_allocates_each_byte_of_the_file() {
    const LEN: u64 = 64 << 20;
    run(|files, data| async move {
        create(&files, "a", LEN).await.close().await;
        let allocated = std::fs::metadata(data.join("a")).unwrap().blocks() * 512;
        assert!(allocated >= LEN, "{allocated} of {LEN} bytes");
    });
}

#[test]
fn create_keeps_the_bytes_of_a_file_that_is_there() {
    run(|files, _| async move {
        let pool = pool();
        let file = create(&files, "a", 4 * KIB).await;
        file.write_at(0, &[block(&pool, b"old")]).await.unwrap();
        file.close().await;
        let file = create(&files, "a", 4 * KIB).await;
        assert_eq!(read(&file, &pool, 0, 3).await, b"old");
    });
}

#[test]
fn create_allocates_an_empty_file_that_is_there() {
    run(|files, data| async move {
        std::fs::write(data.join("a"), b"").unwrap();
        let file = create(&files, "a", 4 * KIB).await;
        assert_eq!(file.len(), 4 * KIB);
    });
}

#[cfg(target_os = "linux")]
#[test]
fn create_frees_the_blocks_past_the_end_of_an_empty_file_that_is_there() {
    run(|files, data| async move {
        crate::kept::check(&files, &data).await;
    });
}

#[test]
fn create_of_no_bytes_makes_an_empty_file() {
    run(|files, data| async move {
        let file = create(&files, "a", 0).await;
        assert_eq!(file.len(), 0);
        assert_eq!(std::fs::metadata(data.join("a")).unwrap().len(), 0);
    });
}

#[test]
fn create_of_another_length_gives_length() {
    run(|files, _| async move {
        create(&files, "a", 4 * KIB).await.close().await;
        let mode = Mode::Create { len: 8 * KIB };
        let found = files.open(Path::new("a"), mode).await.unwrap_err();
        let expected = Error::Length {
            path: "a".into(),
            expected: 8 * KIB,
            found: 4 * KIB,
        };
        assert_eq!(found, expected);
    });
}

/// The file size limit of `a_create_past_the_file_size_limit_leaves_no_file`.
#[cfg(target_os = "linux")]
const LIMIT: u64 = 1 << 20;

#[cfg(target_os = "linux")]
#[test]
fn a_create_past_the_file_size_limit_leaves_no_file() {
    crate::fsize::under(LIMIT, || {
        run(|files, _| async move {
            let mode = Mode::Create { len: 8 * LIMIT };
            let found = files.open(Path::new("a"), mode).await.unwrap_err();
            assert_eq!(found, io("a", Operation::Open, libc::EFBIG));
            let found = files.open(Path::new("a"), Mode::Write).await.unwrap_err();
            assert_eq!(found, Error::NotFound { path: "a".into() });
        });
    });
}

/// A limit that the parent of the test binary set, as `ulimit -f` in a shell does,
/// comes with no ignored `SIGXFSZ`.
#[cfg(target_os = "linux")]
#[test]
fn the_file_size_limit_test_passes_under_a_limit_that_its_parent_set() {
    let script = r#"ulimit -f "$0" && exec "$1" --exact "$2" --nocapture"#;
    let output = std::process::Command::new("sh")
        .args(["-c", script])
        .arg((LIMIT / 512).to_string())
        .arg(std::env::current_exe().unwrap())
        .arg("files::a_create_past_the_file_size_limit_leaves_no_file")
        .output()
        .unwrap();
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{}: {error}", output.status);
    // A name that matches no test also ends with success.
    let ran = String::from_utf8_lossy(&output.stdout);
    assert!(ran.contains("test result: ok. 1 passed"), "{ran}");
}

#[test]
fn an_open_of_a_missing_file_gives_not_found() {
    run(|files, _| async move {
        for mode in [Mode::Read, Mode::Write] {
            let found = files.open(Path::new("a"), mode).await.unwrap_err();
            assert_eq!(found, Error::NotFound { path: "a".into() });
        }
        let found = files.open(Path::new("b/a"), Mode::Create { len: KIB });
        let expected = Error::NotFound { path: "b/a".into() };
        assert_eq!(found.await.unwrap_err(), expected);
    });
}

#[test]
fn open_absolute_reads_a_file_outside_the_data_directory() {
    run(|files, data| async move {
        let path = data.parent().unwrap().join("token");
        std::fs::write(&path, b"abc").unwrap();
        let file = files.open_absolute(&absolute(&path)).await.unwrap();
        assert_eq!(read(&file, &pool(), 0, 3).await, b"abc");
        let gone = data.parent().unwrap().join("gone");
        let found = files.open_absolute(&absolute(&gone)).await.unwrap_err();
        assert_eq!(found, Error::NotFound { path: gone });
    });
}

/// A write open of a mode 0444 file gives `EACCES` to a user other than root, and
/// its lock gives `Busy` to a second open.
#[test]
fn open_absolute_opens_to_read_only_with_no_lock() {
    run(|files, data| async move {
        let path = data.parent().unwrap().join("token");
        std::fs::write(&path, b"abc").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444))
            .unwrap();
        let (other, thread) = self::files(data.parent().unwrap(), "other");
        let held = files.open_absolute(&absolute(&path)).await.unwrap();
        let second = other.open_absolute(&absolute(&path)).await.unwrap();
        let pool = pool();
        assert_eq!(read(&held, &pool, 0, 3).await, b"abc");
        assert_eq!(read(&second, &pool, 0, 3).await, b"abc");
        drop((held, second, other));
        thread.join().unwrap();
    });
}

/// A FIFO with no writer would block an open with no `NONBLOCK`, and each later call
/// with it, so the opens run on a thread that the test gives 5 s.
#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "a blocked open must not hang the test"
)]
fn open_absolute_refuses_a_file_that_is_not_regular_and_waits_for_no_writer() {
    let scratch = Scratch::new();
    let (fifo, dir) = (scratch.0.join("fifo"), scratch.0.clone());
    mkfifo(&fifo);
    let (done, ended) = std::sync::mpsc::channel();
    let opens = std::thread::spawn(move || {
        let (files, thread) = files(&dir, "files");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut found = Vec::new();
            for path in [dir.join("fifo"), dir.clone(), PathBuf::from("/dev/null")] {
                found.push(files.open_absolute(&absolute(&path)).await.map(drop));
            }
            files.free().await.unwrap();
            done.send(found).unwrap();
        });
        drop(files);
        thread.join().unwrap();
    });
    let found = ended.recv_timeout(std::time::Duration::from_secs(5));
    if found.is_err() {
        let flags = rustix::fs::OFlags::WRONLY | rustix::fs::OFlags::NONBLOCK;
        drop(rustix::fs::open(&fifo, flags, rustix::fs::Mode::empty()));
    }
    opens.join().unwrap();
    let io = |path: PathBuf, code| {
        Err(Error::Io {
            path,
            operation: Operation::Open,
            code,
        })
    };
    let expected = vec![
        io(fifo, 22),
        io(scratch.0.clone(), 21),
        io("/dev/null".into(), 22),
    ];
    assert_eq!(found.expect("no open blocks"), expected);
}

#[test]
fn a_write_open_of_a_held_file_gives_busy_and_a_read_open_does_not() {
    run(|files, data| async move {
        let held = create(&files, "a", 4 * KIB).await;
        let (other, thread) = self::files(data.parent().unwrap(), "other");
        for files in [&files, &other] {
            for mode in [Mode::Write, Mode::Create { len: 4 * KIB }] {
                let found = files.open(Path::new("a"), mode).await.unwrap_err();
                assert_eq!(found, Error::Busy { path: "a".into() });
            }
            files.open(Path::new("a"), Mode::Read).await.unwrap();
        }
        drop((held, other));
        thread.join().unwrap();
    });
}

#[test]
fn a_write_open_after_a_close_or_a_drop_of_the_holder_succeeds() {
    run(|files, _| async move {
        create(&files, "a", 4 * KIB).await.close().await;
        let file = files.open(Path::new("a"), Mode::Write).await.unwrap();
        drop(file);
        files.open(Path::new("a"), Mode::Write).await.unwrap();
    });
}

#[test]
fn a_rename_moves_the_file_and_the_handle_follows_it() {
    run(|files, _| async move {
        let pool = pool();
        files.create_dir(Path::new("d")).await.unwrap();
        let mut file = create(&files, "d/a", 4 * KIB).await;
        file.write_at(0, &[block(&pool, b"one")]).await.unwrap();
        file.rename(Path::new("d/b")).await.unwrap();
        let found = files.open(Path::new("d/a"), Mode::Read).await.unwrap_err();
        assert_eq!(found, Error::NotFound { path: "d/a".into() });
        file.write_at(3, &[block(&pool, b"two")]).await.unwrap();
        let moved = files.open(Path::new("d/b"), Mode::Read).await.unwrap();
        assert_eq!(read(&moved, &pool, 0, 6).await, b"onetwo");
    });
}

#[test]
fn a_rename_to_a_taken_name_spelled_with_a_dot_gives_exists() {
    run(|files, _| async move {
        drop(create(&files, "b", 4 * KIB).await);
        let mut file = create(&files, "a", 4 * KIB).await;
        let found = file.rename(Path::new("./b")).await.unwrap_err();
        assert_eq!(found, Error::Exists { path: "./b".into() });
        let names = files.list(Path::new("")).await.unwrap();
        assert_eq!(names, [PathBuf::from("a"), PathBuf::from("b")]);
    });
}

#[test]
fn a_path_with_a_trailing_slash_names_only_a_directory() {
    run(|files, _| async move {
        drop(create(&files, "a", 4 * KIB).await);
        let mut results = Vec::new();
        for (path, mode) in [
            ("a/", Mode::Write),
            ("a/.", Mode::Read),
            ("a//", Mode::Read),
            ("a/", Mode::Create { len: 4 * KIB }),
            ("b/", Mode::Create { len: 4 * KIB }),
            ("b/", Mode::Read),
            ("b/.", Mode::Read),
        ] {
            results.push(files.open(Path::new(path), mode).await.map(drop));
        }
        results.push(files.remove(Path::new("a/")).await);
        let names = files.list(Path::new("")).await.unwrap();
        assert_eq!(
            names,
            [PathBuf::from("a")],
            "a refused remove keeps the file"
        );
        for path in ["b/", "a"] {
            results.push(files.remove(Path::new(path)).await);
        }
        // A create with a trailing slash: Linux gives `EISDIR`, macOS gives
        // `ENOTDIR` for a file and `ENOENT` for no file.
        #[cfg(target_os = "macos")]
        let [file, none] = [
            Err(io("a/", Operation::Open, 20)),
            Err(Error::NotFound { path: "b/".into() }),
        ];
        #[cfg(not(target_os = "macos"))]
        let [file, none] = [
            Err(io("a/", Operation::Open, 21)),
            Err(io("b/", Operation::Open, 21)),
        ];
        let expected = [
            Err(io("a/", Operation::Open, 20)),
            Err(io("a/.", Operation::Open, 20)),
            Err(io("a//", Operation::Open, 20)),
            file,
            none,
            Err(Error::NotFound { path: "b/".into() }),
            Err(Error::NotFound { path: "b/.".into() }),
            Err(io("a/", Operation::Remove, 20)),
            Ok(()),
            Ok(()),
        ];
        assert_eq!(results, expected);
    });
}

/// What `unlink` of a directory gives.
#[cfg(target_os = "macos")]
const UNLINK_DIRECTORY: i32 = 1;
#[cfg(not(target_os = "macos"))]
const UNLINK_DIRECTORY: i32 = 21;

#[test]
fn a_path_of_the_data_directory_names_no_file() {
    run(|files, _| async move {
        let mut results = Vec::new();
        for (path, mode) in [
            ("./", Mode::Read),
            ("./", Mode::Write),
            (".", Mode::Read),
            (".", Mode::Create { len: 1 }),
        ] {
            results.push(files.open(Path::new(path), mode).await.map(drop));
        }
        results.push(files.remove(Path::new(".")).await);
        let expected = [
            Err(io("./", Operation::Open, 21)),
            Err(io("./", Operation::Open, 21)),
            Err(io(".", Operation::Open, 21)),
            Err(io(".", Operation::Open, 21)),
            Err(io(".", Operation::Remove, UNLINK_DIRECTORY)),
        ];
        assert_eq!(results, expected);
    });
}

#[test]
fn an_empty_path_names_no_file() {
    run(|files, _| async move {
        let mut results = Vec::new();
        for mode in [Mode::Read, Mode::Write, Mode::Create { len: 4 * KIB }] {
            results.push(files.open(Path::new(""), mode).await.map(drop));
        }
        results.push(files.remove(Path::new("")).await);
        let not_found = || Error::NotFound { path: "".into() };
        let expected = [Err(not_found()), Err(not_found()), Err(not_found()), Ok(())];
        assert_eq!(results, expected);
        assert_eq!(
            files.list(Path::new("")).await.unwrap(),
            Vec::<PathBuf>::new()
        );
    });
}

#[test]
fn a_rename_onto_a_file_that_is_there_gives_exists_and_changes_nothing() {
    run(|files, _| async move {
        let pool = pool();
        let mut file = create(&files, "a", 4 * KIB).await;
        file.write_at(0, &[block(&pool, b"one")]).await.unwrap();
        let other = create(&files, "b", 4 * KIB).await;
        other.write_at(0, &[block(&pool, b"two")]).await.unwrap();
        other.close().await;
        let found = file.rename(Path::new("b")).await.unwrap_err();
        assert_eq!(found, Error::Exists { path: "b".into() });
        for (path, bytes) in [("a", b"one"), ("b", b"two")] {
            let kept = files.open(Path::new(path), Mode::Read).await.unwrap();
            assert_eq!(read(&kept, &pool, 0, 3).await, bytes);
        }
        file.rename(Path::new("c")).await.unwrap();
        let moved = files.open(Path::new("c"), Mode::Read).await.unwrap();
        assert_eq!(read(&moved, &pool, 0, 3).await, b"one");
    });
}

#[test]
fn a_rename_of_a_removed_path_gives_not_found_and_changes_nothing() {
    run(|files, _| async move {
        let mut file = create(&files, "a", 4 * KIB).await;
        files.remove(Path::new("a")).await.unwrap();
        let found = file.rename(Path::new("b")).await.unwrap_err();
        assert_eq!(found, Error::NotFound { path: "a".into() });
        let found = files.open(Path::new("b"), Mode::Read).await.unwrap_err();
        assert_eq!(found, Error::NotFound { path: "b".into() });
    });
}

#[test]
fn a_rename_of_a_path_that_names_another_file_gives_not_found() {
    run(|files, _| async move {
        let pool = pool();
        let mut file = create(&files, "a", 4 * KIB).await;
        files.remove(Path::new("a")).await.unwrap();
        let other = create(&files, "a", 4 * KIB).await;
        other.write_at(0, &[block(&pool, b"new")]).await.unwrap();
        let found = file.rename(Path::new("b")).await.unwrap_err();
        assert_eq!(found, Error::NotFound { path: "a".into() });
        let kept = files.open(Path::new("a"), Mode::Read).await.unwrap();
        assert_eq!(read(&kept, &pool, 0, 3).await, b"new");
        let found = files.open(Path::new("b"), Mode::Read).await.unwrap_err();
        assert_eq!(found, Error::NotFound { path: "b".into() });
    });
}

#[test]
fn a_rename_of_a_path_that_is_a_link_to_the_file_gives_not_found() {
    run(|files, data| async move {
        std::fs::write(data.join("t"), [0; 4_096]).unwrap();
        std::os::unix::fs::symlink("t", data.join("a")).unwrap();
        let mut file = files.open(Path::new("a"), Mode::Write).await.unwrap();
        let found = file.rename(Path::new("b")).await.unwrap_err();
        assert_eq!(found, Error::NotFound { path: "a".into() });
        assert!(data.join("t").is_file() && data.join("a").is_symlink());
        assert!(!data.join("b").exists());
    });
}

#[test]
fn a_write_open_of_the_new_name_gives_busy_until_the_handle_closes() {
    run(|files, data| async move {
        let mut file = create(&files, "a", 4 * KIB).await;
        file.rename(Path::new("b")).await.unwrap();
        let (other, thread) = self::files(data.parent().unwrap(), "other");
        for files in [&files, &other] {
            let found = files.open(Path::new("b"), Mode::Write).await.unwrap_err();
            assert_eq!(found, Error::Busy { path: "b".into() });
        }
        file.close().await;
        files.open(Path::new("b"), Mode::Write).await.unwrap();
        drop(other);
        thread.join().unwrap();
    });
}

#[test]
fn an_error_after_a_rename_names_the_new_path() {
    run(|files, data| async move {
        let pool = pool();
        let mut file = create(&files, "a", 4 * KIB).await;
        file.rename(Path::new("b")).await.unwrap();
        std::fs::write(data.join("b"), [7; 3_000]).unwrap();
        let found = file.read_at(2_000, pool.alloc(2_000).unwrap()).await;
        assert_eq!(found.unwrap_err(), io("b", Operation::ReadAt, 5));
    });
}

#[test]
fn a_read_past_the_end_of_a_file_cut_short_gives_eio() {
    run(|files, data| async move {
        let pool = pool();
        let file = create(&files, "a", 4 * KIB).await;
        std::fs::write(data.join("a"), [7; 3_000]).unwrap();
        let found = file.read_at(2_000, pool.alloc(2_000).unwrap()).await;
        assert_eq!(found.unwrap_err(), io("a", Operation::ReadAt, 5));
    });
}

#[test]
fn an_open_of_a_directory_gives_eisdir() {
    run(|files, _| async move {
        files.create_dir(Path::new("d")).await.unwrap();
        for mode in [Mode::Read, Mode::Write, Mode::Create { len: KIB }] {
            let found = files.open(Path::new("d"), mode).await.unwrap_err();
            assert_eq!(found, io("d", Operation::Open, 21));
        }
    });
}

#[test]
fn list_gives_the_names_in_a_directory() {
    run(|files, _| async move {
        files.create_dir(Path::new("d")).await.unwrap();
        create(&files, "d/b", KIB).await;
        create(&files, "d/a", KIB).await;
        files.create_dir(Path::new("d/c")).await.unwrap();
        create(&files, "d/c/x", KIB).await;
        let names = files.list(Path::new("d")).await.unwrap();
        assert_eq!(names, [PathBuf::from("a"), "b".into(), "c".into()]);
        assert_eq!(
            files.list(Path::new("")).await.unwrap(),
            [PathBuf::from("d")]
        );
        let found = files.list(Path::new("e")).await.unwrap_err();
        assert_eq!(found, Error::NotFound { path: "e".into() });
    });
}

#[test]
fn create_dir_of_a_directory_that_is_there_succeeds() {
    run(|files, _| async move {
        files.create_dir(Path::new("d")).await.unwrap();
        files.create_dir(Path::new("d")).await.unwrap();
        assert_eq!(
            files.list(Path::new("")).await.unwrap(),
            [PathBuf::from("d")]
        );
    });
}

#[test]
fn create_dir_over_a_file_or_in_a_missing_parent_fails() {
    run(|files, _| async move {
        create(&files, "a", KIB).await;
        let found = files.create_dir(Path::new("a")).await.unwrap_err();
        assert_eq!(found, io("a", Operation::CreateDir, 17));
        let found = files.create_dir(Path::new("b/c")).await.unwrap_err();
        assert_eq!(found, Error::NotFound { path: "b/c".into() });
    });
}

#[test]
fn remove_removes_a_file() {
    run(|files, _| async move {
        create(&files, "a", KIB).await.close().await;
        files.remove(Path::new("a")).await.unwrap();
        assert!(files.list(Path::new("")).await.unwrap().is_empty());
        files.remove(Path::new("a")).await.unwrap();
    });
}

#[test]
fn a_remove_through_the_handle_removes_the_file_and_a_create_at_its_path_opens() {
    run(|files, _| async move {
        let pool = pool();
        let file = create(&files, "a", 4 * KIB).await;
        file.write_at(0, &[block(&pool, b"old")]).await.unwrap();
        assert_eq!(file.remove().await, Ok(()));
        assert!(files.list(Path::new("")).await.unwrap().is_empty());
        let found = files.open(Path::new("a"), Mode::Write).await.unwrap_err();
        assert_eq!(found, Error::NotFound { path: "a".into() });
        let made = create(&files, "a", KIB).await;
        assert_eq!(read(&made, &pool, 0, 3).await, [0; 3]);
    });
}

#[test]
fn a_remove_of_a_path_that_names_another_file_gives_not_found_and_keeps_it() {
    run(|files, _| async move {
        let pool = pool();
        let file = create(&files, "a", 4 * KIB).await;
        files.remove(Path::new("a")).await.unwrap();
        let other = create(&files, "a", 4 * KIB).await;
        other.write_at(0, &[block(&pool, b"new")]).await.unwrap();
        let found = file.remove().await.unwrap_err();
        assert_eq!(found, Error::NotFound { path: "a".into() });
        let kept = files.open(Path::new("a"), Mode::Read).await.unwrap();
        assert_eq!(read(&kept, &pool, 0, 3).await, b"new");
    });
}

#[test]
fn a_remove_of_a_path_that_is_a_link_to_the_file_gives_not_found() {
    run(|files, data| async move {
        std::fs::write(data.join("t"), [0; 4_096]).unwrap();
        std::os::unix::fs::symlink("t", data.join("a")).unwrap();
        let file = files.open(Path::new("a"), Mode::Write).await.unwrap();
        let found = file.remove().await.unwrap_err();
        assert_eq!(found, Error::NotFound { path: "a".into() });
        assert!(data.join("t").is_file() && data.join("a").is_symlink());
        files.open(Path::new("t"), Mode::Write).await.unwrap();
    });
}

#[test]
fn a_remove_through_the_handle_releases_the_lock_of_the_file() {
    run(|files, data| async move {
        let file = create(&files, "a", 4 * KIB).await;
        std::fs::hard_link(data.join("a"), data.join("b")).unwrap();
        let (other, thread) = self::files(data.parent().unwrap(), "other");
        for files in [&files, &other] {
            let found = files.open(Path::new("b"), Mode::Write).await.unwrap_err();
            assert_eq!(found, Error::Busy { path: "b".into() });
        }
        file.remove().await.unwrap();
        assert!(!data.join("a").exists() && data.join("b").is_file());
        other.open(Path::new("b"), Mode::Write).await.unwrap();
        drop(other);
        thread.join().unwrap();
    });
}

#[test]
#[should_panic(expected = "remove a, which was opened to read")]
fn a_remove_through_a_read_handle_panics() {
    run(|files, _| async move {
        create(&files, "a", KIB).await.close().await;
        let file = files.open(Path::new("a"), Mode::Read).await.unwrap();
        drop(file.remove().await);
    });
}

#[test]
fn sync_dir_syncs_a_directory_that_is_there() {
    run(|files, _| async move {
        files.create_dir(Path::new("d")).await.unwrap();
        files.sync_dir(Path::new("d")).await.unwrap();
        files.sync_dir(Path::new("")).await.unwrap();
        let found = files.sync_dir(Path::new("e")).await.unwrap_err();
        assert_eq!(found, Error::NotFound { path: "e".into() });
    });
}

#[test]
fn a_sync_after_a_write_succeeds() {
    run(|files, _| async move {
        let pool = pool();
        let file = create(&files, "a", 4 * KIB).await;
        file.write_at(0, &[block(&pool, b"x")]).await.unwrap();
        file.sync().await.unwrap();
    });
}

/// The error of `os::files` on `dir`.
#[test]
fn a_disk_shows_as_disk() {
    let scratch = Scratch::new();
    let (disk, thread) =
        os::files(&scratch.0, &os::threads().unwrap(), "files").unwrap();
    assert_eq!(format!("{disk:?}"), "Disk { .. }");
    drop(disk);
    thread.join().unwrap();
}

fn dir_error(dir: &Path) -> os::Error {
    os::files(dir, &os::threads().unwrap(), "files").unwrap_err()
}

#[test]
fn files_of_a_missing_dir_makes_it_with_its_data() {
    let scratch = Scratch::new();
    let dir = scratch.0.join("a");
    let (disk, thread) = os::files(&dir, &os::threads().unwrap(), "files").unwrap();
    assert!(std::fs::metadata(dir.join("data")).unwrap().is_dir());
    let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
    drop(disk);
    thread.join().unwrap();
}

#[test]
fn files_of_a_dir_with_a_missing_parent_gives_dir_and_makes_nothing() {
    let scratch = Scratch::new();
    let found = dir_error(&scratch.0.join("a").join("b"));
    assert_eq!(
        found.to_string(),
        "cannot open, make, or sync the data directory or its parent: No such file or \
         directory (os error 2)"
    );
    assert!(matches!(found, os::Error::Dir(_)));
    assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 0);
}

/// Each call syncs the holders of `dir` and `dir/data`, also when they are there: an
/// entry that a call made before a crash is not durable until then.
#[cfg(target_os = "linux")]
#[test]
fn files_syncs_the_holders_of_dir_and_its_data_at_each_call() {
    use std::sync::{Arc, Mutex};

    let scratch = Scratch::new();
    let root = std::fs::canonicalize(&scratch.0).unwrap();
    let dir = root.join("a");
    let threads = os::threads().unwrap();
    let handle = threads.start("synced", move || async move {
        let synced = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&synced);
        crate::seccomp::answer_calls(&[libc::SYS_fsync], move |data, _| {
            let fd = format!("/proc/self/fd/{}", data.args[0]);
            seen.lock().unwrap().push(std::fs::read_link(fd).unwrap());
            None
        });
        for _ in 0..2 {
            let (disk, thread) = os::files(&dir, &os::threads().unwrap(), "files")
                .expect("the disk opens");
            drop(disk);
            thread.join().unwrap();
        }
        let holders = [root.clone(), dir.clone()];
        assert_eq!(*synced.lock().unwrap(), [holders.clone(), holders].concat());
    });
    crate::panics::assert_joins(handle.unwrap(), Ok(()));
}

/// A `dir` that the call made under a holder that it cannot sync is still not
/// durable, so each later call fails too.
#[test]
fn files_under_a_parent_it_cannot_read_gives_dir_at_each_call() {
    let scratch = Scratch::new();
    let dir = scratch.0.join("a");
    // Write and search only: a read open of the parent fails.
    std::fs::set_permissions(&scratch.0, std::fs::Permissions::from_mode(0o300))
        .unwrap();
    let found = [dir_error(&dir).to_string(), dir_error(&dir).to_string()];
    std::fs::set_permissions(&scratch.0, std::fs::Permissions::from_mode(0o700))
        .unwrap();
    let denied = "cannot open, make, or sync the data directory or its parent: \
                  Permission denied (os error 13)";
    assert_eq!(found, [denied, denied]);
}

#[test]
fn files_of_a_missing_dir_under_a_parent_it_cannot_write_gives_the_cause() {
    let scratch = Scratch::new();
    std::fs::set_permissions(&scratch.0, std::fs::Permissions::from_mode(0o500))
        .unwrap();
    let found = dir_error(&scratch.0.join("a"));
    std::fs::set_permissions(&scratch.0, std::fs::Permissions::from_mode(0o700))
        .unwrap();
    assert_eq!(
        found.to_string(),
        "cannot open, make, or sync the data directory or its parent: Permission \
         denied (os error 13)"
    );
    assert!(matches!(found, os::Error::Dir(_)));
}

#[cfg(target_os = "linux")]
#[test]
fn files_whose_holder_sync_fails_gives_dir() {
    let scratch = Scratch::new();
    let dir = scratch.0.join("a");
    let threads = os::threads().unwrap();
    let handle = threads.start("synced", move || async move {
        crate::seccomp::answer_calls(&[libc::SYS_fsync], |_, k| {
            (k == 0).then_some(libc::EIO)
        });
        let found = dir_error(&dir);
        assert_eq!(
            found.to_string(),
            "cannot open, make, or sync the data directory or its parent: \
             Input/output error (os error 5)"
        );
        assert!(matches!(found, os::Error::Dir(_)));
    });
    crate::panics::assert_joins(handle.unwrap(), Ok(()));
}

#[test]
fn files_in_a_directory_under_a_file_gives_dir() {
    let scratch = Scratch::new();
    std::fs::write(scratch.0.join("a"), b"").unwrap();
    let found = dir_error(&scratch.0.join("a"));
    assert_eq!(
        found.to_string(),
        "cannot open, make, or sync the data directory or its parent: Not a directory \
         (os error 20)"
    );
    assert!(matches!(found, os::Error::Dir(_)));
}

#[test]
fn a_write_open_while_another_create_fails_holds_the_file_at_its_path() {
    hold_while_another_create_fails(Mode::Write);
}

#[test]
fn a_create_while_another_create_fails_holds_the_file_at_its_path() {
    hold_while_another_create_fails(Mode::Create { len: 4 * KIB });
}

/// Opens `a` with `mode` while another handle's creates fail and unlink it, and
/// checks that the open holds the file at its path.
fn hold_while_another_create_fails(mode: Mode) {
    let scratch = Scratch::new();
    let (creator, creator_thread) = files(&scratch.0, "creator");
    let (writer, writer_thread) = files(&scratch.0, "writer");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        for _ in 0..500 {
            let stopped = Cell::new(false);
            // Fails with no allocation, so each create unlinks the file it made.
            let mut create = pin!(async {
                while !stopped.get() {
                    let mode = Mode::Create { len: u64::MAX };
                    creator.open(Path::new("a"), mode).await.unwrap_err();
                }
            });
            let mut write = pin!(async {
                loop {
                    if let Ok(file) = writer.open(Path::new("a"), mode).await {
                        stopped.set(true);
                        return file;
                    }
                }
            });
            let mut file = None;
            poll_fn(|context| {
                if file.is_none()
                    && let Poll::Ready(found) = write.as_mut().poll(context)
                {
                    file = Some(found);
                }
                match (file.is_some(), create.as_mut().poll(context)) {
                    (true, Poll::Ready(())) => Poll::Ready(()),
                    _ => Poll::Pending,
                }
            })
            .await;
            let found = writer.open(Path::new("a"), Mode::Write).await.unwrap_err();
            assert_eq!(found, Error::Busy { path: "a".into() });
            file.unwrap().close().await;
            writer.remove(Path::new("a")).await.unwrap();
        }
    });
    drop((creator, writer));
    creator_thread.join().unwrap();
    writer_thread.join().unwrap();
}

/// The `ftruncate` that frees the blocks of a failed create also fails.
#[cfg(target_os = "linux")]
#[test]
fn a_failed_create_whose_free_fails_leaves_no_file() {
    let scratch = Scratch::new();
    let dir = scratch.0.clone();
    let threads = os::threads().unwrap();
    let handle = threads.start("freed", move || async move {
        crate::seccomp::answer_calls(
            &[libc::SYS_fallocate, libc::SYS_ftruncate],
            |data, k| match i64::from(data.nr) {
                libc::SYS_fallocate => Some(libc::ENOSPC),
                libc::SYS_ftruncate if k == 1 => Some(libc::EIO),
                _ => None,
            },
        );
        let (files, thread) = files(&dir, "files");
        let mode = Mode::Create { len: 4 * KIB };
        let created = files.open(Path::new("a"), mode).await.map(|_| ());
        let reopened = files.open(Path::new("a"), Mode::Write).await.map(|_| ());
        drop(files);
        thread.join().unwrap();
        assert_eq!(created, Err(io("a", Operation::Open, libc::EIO)));
        assert_eq!(reopened, Err(Error::NotFound { path: "a".into() }));
    });
    crate::panics::assert_joins(handle.unwrap(), Ok(()));
}

/// Makes each unlock by `flock` of this thread, or of a thread that it starts later,
/// give `errno`.
#[cfg(target_os = "linux")]
fn unlock_gives(errno: i32) {
    crate::seccomp::answer_calls(&[libc::SYS_flock], move |data, _| {
        (data.args[1] == libc::LOCK_UN as u64).then_some(errno)
    });
}

/// The unlock of a closed write handle fails, as an NFS `LOCKU` or a FUSE daemon can.
#[cfg(target_os = "linux")]
#[test]
fn a_close_whose_unlock_fails_closes_the_file() {
    let scratch = Scratch::new();
    let dir = scratch.0.clone();
    let threads = os::threads().unwrap();
    let handle = threads.start("unlock", move || async move {
        unlock_gives(libc::EIO);
        let (files, thread) = files(&dir, "files");
        let file = create(&files, "a", 4 * KIB).await;
        file.close().await;
        let reopened = files.open(Path::new("a"), Mode::Write).await.map(|_| ());
        drop(files);
        thread.join().unwrap();
        assert_eq!(reopened, Ok(()));
    });
    crate::panics::assert_joins(handle.unwrap(), Ok(()));
}

/// An unlock that gives `EBADF` is a broken invariant: the close panics the I/O thread.
#[cfg(target_os = "linux")]
#[test]
fn a_close_whose_unlock_gives_ebadf_panics_the_io_thread() {
    let scratch = Scratch::new();
    let dir = scratch.0.clone();
    let threads = os::threads().unwrap();
    let (sent, io) = std::sync::mpsc::channel();
    let handle = threads.start("unlock", move || async move {
        unlock_gives(libc::EBADF);
        let (files, thread) = files(&dir, "files");
        let file = create(&files, "a", 4 * KIB).await;
        sent.send(thread).unwrap();
        file.close().await;
    });
    crate::panics::assert_joins(handle.unwrap(), crate::panics::panicked("unlock"));
    let thread = io.try_recv().unwrap();
    crate::panics::assert_joins(thread, crate::panics::panicked("files"));
}

/// An unlock that gives `EINVAL` is a broken invariant too: the close panics the I/O
/// thread.
#[cfg(target_os = "linux")]
#[test]
fn a_close_whose_unlock_gives_einval_panics_the_io_thread() {
    let scratch = Scratch::new();
    let dir = scratch.0.clone();
    let threads = os::threads().unwrap();
    let (sent, io) = std::sync::mpsc::channel();
    let handle = threads.start("unlock", move || async move {
        unlock_gives(libc::EINVAL);
        let (files, thread) = files(&dir, "files");
        let file = create(&files, "a", 4 * KIB).await;
        sent.send(thread).unwrap();
        file.close().await;
    });
    crate::panics::assert_joins(handle.unwrap(), crate::panics::panicked("unlock"));
    let thread = io.try_recv().unwrap();
    crate::panics::assert_joins(thread, crate::panics::panicked("files"));
}

/// A write open that another disk holds the lock of gives `Busy`, holds no lock, and so
/// sends no unlock.
#[cfg(target_os = "linux")]
#[test]
fn a_busy_write_open_sends_no_unlock() {
    let scratch = Scratch::new();
    let (holder, holder_thread) = files(&scratch.0, "holder");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let held = runtime.block_on(create(&holder, "a", KIB));
    let dir = scratch.0.clone();
    let threads = os::threads().unwrap();
    let handle = threads.start("unlock", move || async move {
        unlock_gives(libc::EBADF);
        let (files, thread) = files(&dir, "files");
        let opened = files.open(Path::new("a"), Mode::Write).await.map(drop);
        drop(files);
        thread.join().unwrap();
        assert_eq!(opened, Err(Error::Busy { path: "a".into() }));
    });
    crate::panics::assert_joins(handle.unwrap(), Ok(()));
    runtime.block_on(held.close());
    drop(holder);
    holder_thread.join().unwrap();
}

/// A read handle holds no lock, so its close sends no unlock.
#[cfg(target_os = "linux")]
#[test]
fn the_close_of_a_read_handle_sends_no_unlock() {
    let scratch = Scratch::new();
    std::fs::create_dir_all(scratch.0.join("data")).unwrap();
    std::fs::write(scratch.0.join("data/a"), b"").unwrap();
    let dir = scratch.0.clone();
    let threads = os::threads().unwrap();
    let handle = threads.start("unlock", move || async move {
        unlock_gives(libc::EBADF);
        let (files, thread) = files(&dir, "files");
        let read = files.open(Path::new("a"), Mode::Read).await.unwrap();
        read.close().await;
        drop(files);
        thread.join().unwrap();
    });
    crate::panics::assert_joins(handle.unwrap(), Ok(()));
}

/// A remove that drops while it waits for room in the full queue of the I/O thread
/// never runs.
#[test]
fn a_remove_that_drops_while_it_waits_for_room_leaves_the_file() {
    run(|files, data| async move {
        create(&files, "a", KIB).await.close().await;
        let frees = stalled(&files, &data, |context| {
            // 64 is the depth of the queue of the I/O thread.
            let mut frees: Vec<_> = (0..64).map(|_| Box::pin(files.free())).collect();
            for free in &mut frees {
                pend(free, context);
            }
            let mut remove = Box::pin(files.remove(Path::new("a")));
            pend(&mut remove, context);
            drop(remove);
            frees
        })
        .await;
        for free in frees {
            free.await.unwrap();
        }
        files.free().await.unwrap();
        let found = files.open(Path::new("a"), Mode::Read).await.map(drop);
        assert_eq!(found, Ok(()));
    });
}

/// Runs `start` while the I/O thread of `files` blocks in the open of a FIFO, so each
/// call that `start` polls once waits in the queue, and then frees the thread.
async fn stalled<T>(
    files: &Files,
    data: &Path,
    start: impl FnOnce(&mut std::task::Context<'_>) -> T,
) -> T {
    mkfifo(&data.join("p"));
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    let mut blocker = Box::pin(files.open(Path::new("p"), Mode::Read));
    assert!(blocker.as_mut().poll(&mut context).is_pending());
    let started = start(&mut context);
    let writer = rustix::fs::open(
        data.join("p"),
        rustix::fs::OFlags::WRONLY,
        rustix::fs::Mode::empty(),
    );
    drop(blocker.await.unwrap());
    drop(writer.unwrap());
    std::fs::remove_file(data.join("p")).unwrap();
    started
}

/// Makes a FIFO at `path`.
pub(crate) fn mkfifo(path: &Path) {
    #[cfg(target_os = "linux")]
    {
        let mode = rustix::fs::Mode::from_raw_mode(0o600);
        rustix::fs::mkfifoat(rustix::fs::CWD, path, mode).unwrap();
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: `path` ends in a NUL and lives past the call.
        #[expect(unsafe_code, reason = "`rustix` has no `mkfifoat` on macOS")]
        let made = unsafe { libc::mkfifo(path.as_ptr(), 0o600) };
        assert_eq!(made, 0, "{}", std::io::Error::last_os_error());
    }
}

/// Polls `future` once, and expects it to wait.
fn pend<F: Future>(
    future: &mut std::pin::Pin<Box<F>>,
    context: &mut std::task::Context<'_>,
) {
    assert!(future.as_mut().poll(context).is_pending());
}

#[test]
fn a_write_open_waits_for_the_write_of_a_dropped_handle() {
    run(|files, data| async move {
        let (file, pool) = (create(&files, "a", KIB).await, pool());
        let parts = [block(&pool, &[1; 1_024])];
        let mut open = stalled(&files, &data, |context| {
            let mut write = Box::pin(file.write_at(0, &parts));
            pend(&mut write, context);
            drop(write);
            drop(file);
            let mut open = Box::pin(files.open(Path::new("a"), Mode::Write));
            pend(&mut open, context);
            open
        })
        .await;
        let file = open.as_mut().await.unwrap();
        assert_eq!(read(&file, &pool, 0, 4).await, [1; 4]);
        let second = files.open(Path::new("a"), Mode::Write).await.map(drop);
        assert_eq!(second, Err(Error::Busy { path: "a".into() }));
    });
}

#[test]
fn a_write_open_waits_for_the_write_of_a_dropped_handle_of_a_removed_file() {
    run(|files, data| async move {
        let pool = pool();
        let big = pool.largest();
        let file = create(&files, "a", big as u64).await;
        files.remove(Path::new("a")).await.unwrap();
        let parts = [pool.alloc(big).unwrap().freeze()];
        let mut open = stalled(&files, &data, |context| {
            let mut write = Box::pin(file.write_at(0, &parts));
            pend(&mut write, context);
            drop(write);
            drop(file);
            drop(parts);
            pool.alloc(big).unwrap_err();
            let mut open =
                Box::pin(files.open(Path::new("a"), Mode::Create { len: KIB }));
            pend(&mut open, context);
            open
        })
        .await;
        drop(open.as_mut().await.unwrap());
        drop(pool.alloc(big).unwrap());
    });
}

#[test]
fn a_write_open_waits_for_the_calls_of_a_dropped_close() {
    run(|files, data| async move {
        let (file, pool) = (create(&files, "a", KIB).await, pool());
        let parts = [block(&pool, &[1; 1_024])];
        let mut open = stalled(&files, &data, |context| {
            let mut write = Box::pin(file.write_at(0, &parts));
            pend(&mut write, context);
            drop(write);
            let mut close = Box::pin(file.close());
            pend(&mut close, context);
            drop(close);
            let mut open = Box::pin(files.open(Path::new("a"), Mode::Write));
            pend(&mut open, context);
            open
        })
        .await;
        assert_eq!(open.as_mut().await.map(drop), Ok(()));
    });
}

#[test]
fn a_write_open_waits_for_a_dropped_call_on_the_file_that_its_path_names() {
    run(|files, data| async move {
        let (mut file, pool) = (create(&files, "a", KIB).await, pool());
        let parts = [block(&pool, &[1; 512])];
        let mut renamed = stalled(&files, &data, |context| {
            let mut write = Box::pin(file.write_at(0, &parts));
            pend(&mut write, context);
            drop(write);
            let mut renamed = Box::pin(file.rename(Path::new("b")));
            pend(&mut renamed, context);
            renamed
        })
        .await;
        assert_eq!(renamed.as_mut().await, Ok(()));
        drop(renamed);
        drop(file);
        let found = files.open(Path::new("b"), Mode::Write).await.map(drop);
        assert_eq!(found, Ok(()));
    });
}

#[test]
fn a_create_waits_for_a_dropped_read_of_a_reader_on_its_path_after_a_rename() {
    run(|files, data| async move {
        let pool = pool();
        let big = pool.largest();
        let mut writer = create(&files, "a", big as u64).await;
        let reader = files.open(Path::new("a"), Mode::Read).await.unwrap();
        let mut renamed = stalled(&files, &data, |context| {
            let mut read = Box::pin(reader.read_at(0, pool.alloc(big).unwrap()));
            pend(&mut read, context);
            drop(read);
            drop(reader);
            pool.alloc(big).unwrap_err();
            let mut renamed = Box::pin(writer.rename(Path::new("b")));
            pend(&mut renamed, context);
            renamed
        })
        .await;
        assert_eq!(renamed.as_mut().await, Ok(()));
        drop(renamed);
        drop(writer);
        drop(create(&files, "a", KIB).await);
        drop(pool.alloc(big).unwrap());
    });
}

#[test]
fn a_create_waits_for_a_dropped_read_of_a_stale_reader_of_a_removed_file() {
    run(|files, data| async move {
        let pool = pool();
        let big = pool.largest();
        let mut writer = create(&files, "a", big as u64).await;
        let reader = files.open(Path::new("a"), Mode::Read).await.unwrap();
        writer.rename(Path::new("b")).await.unwrap();
        let mut remove = stalled(&files, &data, |context| {
            let mut remove = Box::pin(files.remove(Path::new("b")));
            pend(&mut remove, context);
            let mut read = Box::pin(reader.read_at(0, pool.alloc(big).unwrap()));
            pend(&mut read, context);
            drop(read);
            drop(reader);
            pool.alloc(big).unwrap_err();
            remove
        })
        .await;
        assert_eq!(remove.as_mut().await, Ok(()));
        drop(remove);
        drop(writer);
        drop(create(&files, "b", KIB).await);
        drop(pool.alloc(big).unwrap());
    });
}

#[test]
fn a_write_open_waits_for_a_dropped_rename_to_its_path() {
    run(|files, data| async move {
        let mut file = create(&files, "a", KIB).await;
        // The drop of this future drops the rename and then the file. The first poll
        // starts the sync of the rename; the second, after the sync ends, starts the
        // rename itself.
        let mut rename = Box::pin(async move { file.rename(Path::new("b")).await });
        stalled(&files, &data, |context| pend(&mut rename, context)).await;
        files.free().await.unwrap();
        let mut open = stalled(&files, &data, |context| {
            pend(&mut rename, context);
            drop(rename);
            let mut open = Box::pin(files.open(Path::new("b"), Mode::Write));
            pend(&mut open, context);
            open
        })
        .await;
        assert_eq!(open.as_mut().await.map(drop), Ok(()));
        let names = files.list(Path::new("")).await.unwrap();
        assert_eq!(names, [PathBuf::from("b")]);
    });
}

#[test]
fn a_create_after_a_dropped_remove_keeps_its_file() {
    run(|files, data| async move {
        create(&files, "a", KIB).await.close().await;
        let mut made = stalled(&files, &data, |context| {
            let mut remove = Box::pin(files.remove(Path::new("a")));
            pend(&mut remove, context);
            drop(remove);
            let mut made =
                Box::pin(files.open(Path::new("a"), Mode::Create { len: KIB }));
            pend(&mut made, context);
            made
        })
        .await;
        made.as_mut().await.unwrap().close().await;
        let names = files.list(Path::new("")).await.unwrap();
        assert_eq!(names, [PathBuf::from("a")]);
    });
}

#[test]
fn a_write_open_waits_for_a_dropped_remove_and_then_a_create_stays() {
    run(|files, data| async move {
        let file = create(&files, "a", KIB).await;
        let mut open = stalled(&files, &data, |context| {
            let mut remove = Box::pin(file.remove());
            pend(&mut remove, context);
            drop(remove);
            let mut open = Box::pin(files.open(Path::new("a"), Mode::Write));
            pend(&mut open, context);
            open
        })
        .await;
        let found = open.as_mut().await.map(drop);
        assert_eq!(found, Err(Error::NotFound { path: "a".into() }));
        create(&files, "a", KIB).await.close().await;
        let names = files.list(Path::new("")).await.unwrap();
        assert_eq!(names, [PathBuf::from("a")]);
    });
}

#[test]
fn a_rename_waits_for_a_dropped_remove_of_its_new_name() {
    run(|files, data| async move {
        create(&files, "b", KIB).await.close().await;
        let mut file = create(&files, "a", KIB).await;
        let mut renamed = stalled(&files, &data, |context| {
            let mut remove = Box::pin(files.remove(Path::new("b")));
            pend(&mut remove, context);
            drop(remove);
            let mut renamed = Box::pin(file.rename(Path::new("b")));
            pend(&mut renamed, context);
            renamed
        })
        .await;
        assert_eq!(renamed.as_mut().await, Ok(()));
        let names = files.list(Path::new("")).await.unwrap();
        assert_eq!(names, [PathBuf::from("b")]);
    });
}

#[test]
fn a_write_open_waits_for_a_remove_through_the_handle_whose_future_lives() {
    run(|files, data| async move {
        let file = create(&files, "a", KIB).await;
        let (mut remove, mut open) = stalled(&files, &data, |context| {
            let mut remove = Box::pin(file.remove());
            pend(&mut remove, context);
            let mut open = Box::pin(files.open(Path::new("a"), Mode::Write));
            pend(&mut open, context);
            (remove, open)
        })
        .await;
        let found = open.as_mut().await.map(drop);
        assert_eq!(found, Err(Error::NotFound { path: "a".into() }));
        assert_eq!(remove.as_mut().await, Ok(()));
    });
}

#[test]
fn a_write_open_waits_for_the_close_of_a_failed_remove_through_the_handle() {
    run(|files, data| async move {
        files.create_dir(Path::new("d")).await.unwrap();
        let file = create(&files, "d/a", KIB).await;
        let locked = std::fs::Permissions::from_mode(0o500);
        std::fs::set_permissions(data.join("d"), locked).unwrap();
        let (mut remove, mut open) = stalled(&files, &data, |context| {
            let mut remove = Box::pin(file.remove());
            pend(&mut remove, context);
            let mut open = Box::pin(files.open(Path::new("d/a"), Mode::Write));
            pend(&mut open, context);
            (remove, open)
        })
        .await;
        let found = open.as_mut().await.map(drop);
        let removed = remove.as_mut().await;
        let open = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(data.join("d"), open).unwrap();
        assert_eq!(removed, Err(io("d/a", Operation::Remove, 13)));
        assert_eq!(found, Ok(()));
    });
}

#[test]
fn a_remove_waits_for_a_dropped_create() {
    run(|files, data| async move {
        let mut removed = stalled(&files, &data, |context| {
            let mode = Mode::Create { len: KIB };
            let mut create = Box::pin(files.open(Path::new("a"), mode));
            pend(&mut create, context);
            drop(create);
            let mut remove = Box::pin(files.remove(Path::new("a")));
            pend(&mut remove, context);
            remove
        })
        .await;
        assert_eq!(removed.as_mut().await, Ok(()));
        assert!(files.list(Path::new("")).await.unwrap().is_empty());
    });
}

#[test]
fn a_file_or_directory_that_is_there_keeps_its_mode() {
    let scratch = Scratch::new();
    let data = scratch.0.join("data");
    std::fs::create_dir_all(data.join("d")).unwrap();
    let len = usize::try_from(KIB).unwrap();
    std::fs::write(data.join("f"), vec![0; len]).unwrap();
    for (path, mode) in [("", 0o755), ("f", 0o644), ("d", 0o755)] {
        let mode = std::fs::Permissions::from_mode(mode);
        std::fs::set_permissions(data.join(path), mode).unwrap();
    }
    assert_eq!(opened(&scratch.0), [0o755, 0o644, 0o755]);
}

#[cfg(target_os = "linux")]
#[test]
fn a_new_directory_takes_the_setgid_bit_of_its_parent() {
    let scratch = Scratch::new();
    let mode = std::fs::Permissions::from_mode(0o2750);
    std::fs::set_permissions(&scratch.0, mode).unwrap();
    assert_eq!(opened(&scratch.0), [0o2700, 0o600, 0o2700]);
}

#[test]
fn a_remove_through_the_handle_ends_after_the_dropped_write_of_its_handle() {
    run(|files, data| async move {
        let pool = pool();
        let big = pool.largest();
        let file = create(&files, "a", big as u64).await;
        files.remove(Path::new("a")).await.unwrap();
        let parts = [pool.alloc(big).unwrap().freeze()];
        let mut remove = stalled(&files, &data, |context| {
            let mut write = Box::pin(file.write_at(0, &parts));
            pend(&mut write, context);
            drop(write);
            drop(parts);
            pool.alloc(big).unwrap_err();
            let mut remove = Box::pin(file.remove());
            pend(&mut remove, context);
            remove
        })
        .await;
        let found = remove.as_mut().await;
        assert_eq!(found, Err(Error::NotFound { path: "a".into() }));
        drop(pool.alloc(big).unwrap());
    });
}

#[test]
fn a_write_open_waits_for_a_remove_through_a_handle_whose_path_names_another_file() {
    run(|files, data| async move {
        let file = create(&files, "a", KIB).await;
        files.remove(Path::new("a")).await.unwrap();
        let (mut remove, mut open) = stalled(&files, &data, |context| {
            let mut remove = Box::pin(file.remove());
            pend(&mut remove, context);
            let mode = Mode::Create { len: KIB };
            let mut open = Box::pin(files.open(Path::new("a"), mode));
            pend(&mut open, context);
            (remove, open)
        })
        .await;
        assert_eq!(open.as_mut().await.map(drop), Ok(()));
        let found = poll_fn(|cx| Poll::Ready(remove.as_mut().poll(cx))).await;
        assert_eq!(
            found,
            Poll::Ready(Err(Error::NotFound { path: "a".into() }))
        );
    });
}

/// The names in `d` from a list that starts, while the I/O thread is stalled, after
/// `drop_call` polls a call once and drops it. `d` is not the data directory, which
/// holds the FIFO of the stall.
async fn list_after_dropped(
    files: &Files,
    data: &Path,
    drop_call: impl FnOnce(&mut std::task::Context<'_>),
) -> Vec<PathBuf> {
    let mut list = stalled(files, data, |context| {
        drop_call(context);
        let mut list = Box::pin(files.list(Path::new("d")));
        pend(&mut list, context);
        list
    })
    .await;
    list.as_mut().await.unwrap()
}

#[test]
fn a_list_waits_for_a_dropped_remove_in_its_directory() {
    run(|files, data| async move {
        files.create_dir(Path::new("d")).await.unwrap();
        create(&files, "d/a", KIB).await.close().await;
        let names = list_after_dropped(&files, &data, |context| {
            pend(&mut Box::pin(files.remove(Path::new("d/a"))), context);
        })
        .await;
        assert_eq!(names, Vec::<PathBuf>::new());
    });
}

#[test]
fn a_list_waits_for_a_dropped_create_in_its_directory() {
    run(|files, data| async move {
        files.create_dir(Path::new("d")).await.unwrap();
        let names = list_after_dropped(&files, &data, |context| {
            let mode = Mode::Create { len: KIB };
            pend(&mut Box::pin(files.open(Path::new("d/a"), mode)), context);
        })
        .await;
        assert_eq!(names, [PathBuf::from("a")]);
    });
}

#[test]
fn a_list_waits_for_a_dropped_rename_to_a_path_in_its_directory() {
    run(|files, data| async move {
        files.create_dir(Path::new("d")).await.unwrap();
        let mut file = create(&files, "d/a", KIB).await;
        // The first poll starts the sync of the rename; the second, after the sync
        // ends, starts the rename itself. The drop drops the rename and then the file.
        let mut rename = Box::pin(async move { file.rename(Path::new("d/b")).await });
        stalled(&files, &data, |context| pend(&mut rename, context)).await;
        files.free().await.unwrap();
        let names = list_after_dropped(&files, &data, |context| {
            pend(&mut rename, context);
            drop(rename);
        })
        .await;
        assert_eq!(names, [PathBuf::from("b")]);
    });
}

#[test]
fn a_list_waits_for_a_dropped_remove_through_a_handle_in_its_directory() {
    run(|files, data| async move {
        files.create_dir(Path::new("d")).await.unwrap();
        let file = create(&files, "d/a", KIB).await;
        let names = list_after_dropped(&files, &data, |context| {
            pend(&mut Box::pin(file.remove()), context);
        })
        .await;
        assert_eq!(names, Vec::<PathBuf>::new());
    });
}

#[test]
fn a_list_waits_for_a_dropped_create_dir_in_its_directory() {
    run(|files, data| async move {
        files.create_dir(Path::new("d")).await.unwrap();
        let names = list_after_dropped(&files, &data, |context| {
            pend(&mut Box::pin(files.create_dir(Path::new("d/e"))), context);
        })
        .await;
        assert_eq!(names, [PathBuf::from("e")]);
    });
}

#[test]
fn stored_of_a_directory_with_no_data_directory_gives_dir_and_makes_nothing() {
    let scratch = Scratch::new();
    let threads = os::threads().unwrap();
    for dir in [scratch.0.join("node"), scratch.0.clone()] {
        let Err(os::Error::Dir(error)) = os::stored(&dir, &threads, "files") else {
            panic!("a directory with no data directory gives Dir");
        };
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }
    let left = std::fs::read_dir(&scratch.0).unwrap().count();
    assert_eq!(left, 0);
}

#[test]
fn stored_reads_the_files_that_files_wrote() {
    let scratch = Scratch::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let pool = pool();
    let (files, thread) = files(&scratch.0, "files");
    runtime.block_on(async {
        let file = create(&files, "a", KIB).await;
        file.write_at(0, &[block(&pool, b"kept")]).await.unwrap();
    });
    drop(files);
    thread.join().unwrap();
    let threads = os::threads().unwrap();
    let (disk, thread) = os::stored(&scratch.0, &threads, "files").unwrap();
    let files = Files::new(disk);
    runtime.block_on(async {
        let file = files.open(Path::new("a"), Mode::Read).await.unwrap();
        assert_eq!(read(&file, &pool, 0, 4).await, b"kept");
    });
    drop(files);
    thread.join().unwrap();
}

#[test]
fn shown_joins_to_the_data_directory() {
    run(|files, data| async move {
        assert_eq!(
            files.shown(Path::new("secrets/a.b")),
            data.join("secrets/a.b")
        );
    });
}

#[test]
fn shown_of_a_relative_directory_is_absolute() {
    let scratch = Scratch::new();
    let cwd = std::env::current_dir().unwrap();
    let up = "../".repeat(cwd.components().count() - 1);
    let dir = Path::new(&up).join(scratch.0.strip_prefix("/").unwrap());
    let threads = os::threads().unwrap();
    let (disk, thread) = os::files(&dir, &threads, "files").unwrap();
    let files = Files::new(disk);
    assert_eq!(files.shown(Path::new("a")), cwd.join(&dir).join("data/a"));
    drop(files);
    thread.join().unwrap();
}

#[test]
fn shown_of_a_stored_directory_joins_to_its_data_directory() {
    let scratch = Scratch::new();
    std::fs::create_dir(scratch.0.join("data")).unwrap();
    let threads = os::threads().unwrap();
    let (disk, thread) = os::stored(&scratch.0, &threads, "files").unwrap();
    let files = Files::new(disk);
    assert_eq!(files.shown(Path::new("a")), scratch.0.join("data/a"));
    drop(files);
    thread.join().unwrap();
}
