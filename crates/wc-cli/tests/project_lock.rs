//! Two wiringcraft processes on one project take turns (#610), whatever their environment, and
//! a command reading its input from a pipe reads it before it waits.

use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn new_project(dir: &Path) -> TestResult {
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    Ok(())
}

/// Wait up to `limit` for the child to end: `true` when it did.
fn ends_within(child: &mut Child, limit: Duration) -> Result<bool, Box<dyn std::error::Error>> {
    let start = Instant::now();
    while start.elapsed() < limit {
        if child.try_wait()?.is_some() {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(false)
}

/// Wait for the thread writing `fifo` to end, at most ten seconds, and return what it
/// returned (`None` when it did not end; an error when the FIFO cannot be opened to drain it). A reader is kept open and drained meanwhile, opened
/// for reading and writing so the open itself never waits: a writer that has yet to open the
/// FIFO, or is blocked writing, gets through whatever the command under test did.
#[cfg(unix)]
fn finish_writer(
    fifo: &Path,
    writer: std::thread::JoinHandle<std::io::Result<()>>,
) -> std::io::Result<Option<std::io::Result<()>>> {
    use std::io::Read;
    let mut reader = std::fs::OpenOptions::new().read(true).write(true).open(fifo)?;
    // Left running when the test ends: it holds only this end of the FIFO.
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while reader.read(&mut buf).is_ok_and(|n| n > 0) {}
    });
    let start = Instant::now();
    while !writer.is_finished() && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(if writer.is_finished() { writer.join().ok() } else { None })
}

#[test]
fn a_command_waits_for_the_lock_another_process_holds() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    new_project(&dir)?;
    // Held as another wiringcraft process would hold it.
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(".wiringcraft.lock"))?;
    lock.lock()?;
    let own_tmp = tmp.path().join("elsewhere");
    std::fs::create_dir(&own_tmp)?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(&dir)
        .args(["rules", "set", "-"])
        // Another temporary folder changes nothing: the lock is in the project.
        .env("TMPDIR", &own_tmp)
        .env("TMP", &own_tmp)
        .env("TEMP", &own_tmp)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let Some(mut input) = child.stdin.take() else {
        unreachable!("stdin is piped");
    };
    // More than a pipe holds: the writer only gets through when the command reads its input
    // while it is still waiting for the lock.
    let mut rules = String::from("[[rule]]\nkind = \"max-wires\"\nmax = 2\n");
    for _ in 0..4096 {
        rules.push_str("# padding to fill the pipe ................................\n");
    }
    assert!(rules.len() > 200_000);
    let writer = std::thread::spawn(move || input.write_all(rules.as_bytes()));
    let start = Instant::now();
    while !writer.is_finished() && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let read_first = writer.is_finished();
    let waits = !ends_within(&mut child, Duration::from_millis(300))?;
    lock.unlock()?;
    let ended = ends_within(&mut child, Duration::from_secs(20))?;
    if !ended {
        child.kill()?;
    }
    // Reaped before any assertion, so a failure leaves no process behind; a killed child also
    // closes its input, which ends the writer.
    let out = child.wait_with_output()?;
    let written = writer.join();
    assert!(read_first, "the input is read before the lock is taken");
    assert!(waits, "it waits for the lock");
    assert!(ended, "it runs once the lock is free");
    let Ok(Ok(())) = written else {
        unreachable!("the input was written");
    };
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(err.contains("waiting for another wiringcraft command"), "{err}");
    let toml = std::fs::read_to_string(dir.join("project.toml"))?;
    assert!(toml.contains("max-wires"), "{toml}");
    Ok(())
}

/// A git hook that runs wiringcraft on the project while `vcs commit` holds the project's lock
/// does not wait for that lock (it would wait for good: the commit waits for the hook).
#[cfg(unix)]
#[test]
fn a_git_hook_running_wiringcraft_on_the_project_does_not_hang_a_commit() -> TestResult {
    hook_commit_ends(false)
}

/// The same with the project named by a relative `-C`, from its parent folder.
#[cfg(unix)]
#[test]
fn a_hook_does_not_hang_a_commit_started_with_a_relative_project_folder() -> TestResult {
    hook_commit_ends(true)
}

#[cfg(unix)]
fn hook_commit_ends(relative: bool) -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .status()?;
    assert!(status.success());
    let hook = dir.join(".git").join("hooks").join("pre-commit");
    std::fs::create_dir_all(hook.parent().ok_or("a folder")?)?;
    let marker = tmp.path().join("hook-ran");
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\n\"{}\" -C . page list > /dev/null && touch \"{}\"\n",
            env!("CARGO_BIN_EXE_wiringcraft"),
            marker.display()
        ),
    )?;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))?;
    let added = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(&dir)
        .args(["page", "add", "p2", "--number", "2"])
        .status()?;
    assert!(added.success());
    let mut commit = Command::new(env!("CARGO_BIN_EXE_wiringcraft"));
    if relative {
        commit.current_dir(tmp.path()).args(["-C", "p"]);
    } else {
        commit.arg("-C").arg(&dir);
    }
    let mut commit = commit
        .args(["vcs", "commit", "-m", "A page"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let ended = ends_within(&mut commit, Duration::from_secs(30))?;
    if !ended {
        commit.kill()?;
    }
    let out = commit.wait_with_output()?;
    assert!(ended, "the commit ends");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(marker.exists(), "the hook ran wiringcraft");
    Ok(())
}

/// A named input file is read before the lock too: fed through a FIFO by a writer that cannot
/// get through until the command opens and reads it.
#[cfg(unix)]
#[test]
fn a_command_reads_a_named_fifo_before_it_waits_for_the_lock() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    new_project(&dir)?;
    let fifo = tmp.path().join("rules.fifo");
    assert!(Command::new("mkfifo").arg(&fifo).status()?.success());
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(".wiringcraft.lock"))?;
    lock.lock()?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(&dir)
        .args(["rules", "set"])
        .arg(&fifo)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let writer = {
        let fifo = fifo.clone();
        std::thread::spawn(move || {
            std::fs::OpenOptions::new()
                .write(true)
                .open(&fifo)?
                .write_all(b"[[rule]]\nkind = \"max-wires\"\nmax = 2\n")
        })
    };
    let start = Instant::now();
    while !writer.is_finished() && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let read_first = writer.is_finished();
    let waits = !ends_within(&mut child, Duration::from_millis(300))?;
    lock.unlock()?;
    let ended = ends_within(&mut child, Duration::from_secs(20))?;
    if !ended {
        child.kill()?;
    }
    let out = child.wait_with_output()?;
    let written = finish_writer(&fifo, writer)?;
    assert!(read_first, "the FIFO is read before the lock is taken");
    assert!(waits, "it waits for the lock");
    assert!(ended, "it runs once the lock is free");
    let Some(Ok(())) = written else {
        unreachable!("the input was written");
    };
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    let toml = std::fs::read_to_string(dir.join("project.toml"))?;
    assert!(toml.contains("max-wires"), "{toml}");
    Ok(())
}

/// `import wago` reads its file before the lock as well, so a FIFO fed by an export of the same
/// project cannot hold both up.
#[cfg(unix)]
#[test]
fn a_wago_import_reads_a_named_fifo_before_it_waits_for_the_lock() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    new_project(&dir)?;
    let exported = tmp.path().join("wago.csv");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(&dir)
        .args(["export", "wago-configurator"])
        .arg(&exported)
        .stdout(Stdio::null())
        .status()?;
    assert!(status.success());
    let content = std::fs::read(&exported)?;
    let fifo = tmp.path().join("wago.fifo");
    assert!(Command::new("mkfifo").arg(&fifo).status()?.success());
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(".wiringcraft.lock"))?;
    lock.lock()?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(&dir)
        .args(["import", "wago"])
        .arg(&fifo)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let writer = {
        let fifo = fifo.clone();
        std::thread::spawn(move || std::fs::OpenOptions::new().write(true).open(&fifo)?.write_all(&content))
    };
    let start = Instant::now();
    while !writer.is_finished() && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let read_first = writer.is_finished();
    let waits = !ends_within(&mut child, Duration::from_millis(300))?;
    lock.unlock()?;
    let ended = ends_within(&mut child, Duration::from_secs(20))?;
    if !ended {
        child.kill()?;
    }
    let out = child.wait_with_output()?;
    let written = finish_writer(&fifo, writer)?;
    assert!(read_first, "the FIFO is read before the lock is taken");
    assert!(waits, "it waits for the lock");
    assert!(ended, "it runs once the lock is free");
    let Some(Ok(())) = written else {
        unreachable!("the input was written");
    };
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    Ok(())
}

/// When the lock cannot be listed in git's exclude list the command still runs, under the
/// lock, and says why the list was left alone.
#[test]
fn an_exclude_list_that_cannot_be_updated_is_reported() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .status()?;
    assert!(status.success());
    let exclude = dir.join(".git").join("info").join("exclude");
    if exclude.exists() {
        std::fs::remove_file(&exclude)?;
    }
    // A folder where the list belongs: it can be neither read nor replaced.
    std::fs::create_dir_all(&exclude)?;
    let out = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(&dir)
        .args(["page", "list"])
        .output()?;
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(err.contains("could not leave .wiringcraft.lock out of git"), "{err}");
    assert!(err.contains("exclude"), "names the list: {err}");
    assert!(exclude.is_dir(), "left as it was");
    Ok(())
}

/// Git variables a caller (a git hook) exports for another repository do not send the lock's
/// exclude entry there: the project's own repository gets it.
#[test]
fn the_lock_is_excluded_in_the_projects_repository_whatever_git_is_pointed_at() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let new_repo = |name: &str| -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
        let dir = tmp.path().join(name);
        let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
            .arg("new")
            .arg(&dir)
            .status()?;
        assert!(status.success());
        // As a repository made before the lock existed.
        std::fs::write(dir.join(".git").join("info").join("exclude"), "# mine\n")?;
        Ok(dir)
    };
    let project = new_repo("p")?;
    let other = new_repo("other")?;
    let out = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(&project)
        .args(["page", "list"])
        .env("GIT_DIR", other.join(".git"))
        .env("GIT_WORK_TREE", &other)
        .env("GIT_INDEX_FILE", other.join(".git").join("index"))
        .output()?;
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let mine = std::fs::read_to_string(project.join(".git").join("info").join("exclude"))?;
    let theirs = std::fs::read_to_string(other.join(".git").join("info").join("exclude"))?;
    assert_eq!(mine, "# mine\n/.wiringcraft.lock\n");
    assert_eq!(theirs, "# mine\n");
    Ok(())
}

/// `import plc-csv` reads its file before the lock too.
#[cfg(unix)]
#[test]
fn a_plc_csv_import_reads_a_named_fifo_before_it_waits_for_the_lock() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    new_project(&dir)?;
    let fifo = tmp.path().join("plc.fifo");
    assert!(Command::new("mkfifo").arg(&fifo).status()?.success());
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(".wiringcraft.lock"))?;
    lock.lock()?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(&dir)
        .args(["import", "plc-csv"])
        .arg(&fifo)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let writer = {
        let fifo = fifo.clone();
        std::thread::spawn(move || {
            std::fs::OpenOptions::new()
                .write(true)
                .open(&fifo)?
                .write_all(b"tag;address\n-K1;%I0.0\n")
        })
    };
    let start = Instant::now();
    while !writer.is_finished() && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let read_first = writer.is_finished();
    let waits = !ends_within(&mut child, Duration::from_millis(300))?;
    lock.unlock()?;
    let ended = ends_within(&mut child, Duration::from_secs(20))?;
    if !ended {
        child.kill()?;
    }
    let out = child.wait_with_output()?;
    let written = finish_writer(&fifo, writer)?;
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(read_first, "the FIFO is read before the lock is taken: {err}");
    assert!(waits, "it waits for the lock: {err}");
    assert!(ended, "it runs once the lock is free");
    let Some(Ok(())) = written else {
        unreachable!("the input was written");
    };
    assert!(!err.contains("cannot read"), "{err}");
    Ok(())
}

/// A symbol drawing small enough to write by hand.
const SVG: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"20\"><rect x=\"0\" y=\"0\" width=\"20\" height=\"20\"/></svg>\n";

/// Run `import symbol-svg` on `file` for the project in `dir`, writing to the library `lib`,
/// and return the symbol file it wrote.
fn import_svg(dir: &Path, file: &Path, lib: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let out = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(dir)
        .args(["import", "symbol-svg"])
        .arg(file)
        .args(["--id", "test.box", "--name", "Box", "--lib"])
        .arg(lib)
        .output()?;
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    Ok(std::fs::read(lib.join("symbols").join("test.box.json"))?)
}

/// A regular input file is read where it is, as before: nothing is copied, and the command
/// names the file itself when it cannot be read.
#[test]
fn a_regular_input_file_is_read_as_before() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    new_project(&dir)?;
    let file = tmp.path().join("box.svg");
    std::fs::write(&file, SVG)?;
    let symbol = import_svg(&dir, &file, &tmp.path().join("lib"))?;
    assert!(String::from_utf8_lossy(&symbol).contains("test.box"));

    let rules = tmp.path().join("rules.toml");
    std::fs::write(&rules, "[[rule]]\nkind = \"max-wires\"\nmax = 2\n")?;
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(&dir)
        .args(["rules", "set"])
        .arg(&rules)
        .stdout(Stdio::null())
        .status()?;
    assert!(status.success());
    assert!(std::fs::read_to_string(dir.join("project.toml"))?.contains("max-wires"));

    let missing = tmp.path().join("missing.svg");
    let out = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(&dir)
        .args(["import", "symbol-svg"])
        .arg(&missing)
        .args(["--id", "test.none", "--name", "None", "--lib"])
        .arg(tmp.path().join("lib"))
        .output()?;
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("missing.svg"), "{err}");
    Ok(())
}

/// What a command reads through a FIFO is what the writer wrote: the symbol comes out the same
/// as from a regular file with the same content.
#[cfg(unix)]
#[test]
fn an_import_through_a_fifo_gives_what_the_file_gives() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    new_project(&dir)?;
    let file = tmp.path().join("box.svg");
    std::fs::write(&file, SVG)?;
    let from_file = import_svg(&dir, &file, &tmp.path().join("lib"))?;
    let fifo = tmp.path().join("fifo.svg");
    assert!(Command::new("mkfifo").arg(&fifo).status()?.success());
    let writer = {
        let fifo = fifo.clone();
        std::thread::spawn(move || {
            std::fs::OpenOptions::new()
                .write(true)
                .open(&fifo)?
                .write_all(SVG.as_bytes())
        })
    };
    let from_fifo = import_svg(&dir, &fifo, &tmp.path().join("lib2"))?;
    let Some(Ok(())) = finish_writer(&fifo, writer)? else {
        unreachable!("the input was written");
    };
    assert_eq!(from_fifo, from_file);
    Ok(())
}

/// A FIFO and the thread that writes to it.
#[cfg(unix)]
type FifoWriter = (std::path::PathBuf, std::thread::JoinHandle<std::io::Result<()>>);

/// Every command that names an input file reads a FIFO before it waits for the lock (#658):
/// a writer that cannot get through until the command opens and reads the FIFO finishes while
/// the lock is still held by another command, and the command then waits for the lock.
/// `FIFO` is the input under test (each one its own FIFO), `FILE` a regular file and `LIB` a
/// library folder. What is written is not a valid input of any of them: each command fails
/// once it has the lock, which is after the point this test is about.
#[cfg(unix)]
#[test]
fn every_command_reads_a_fifo_input_before_it_waits_for_the_lock() -> TestResult {
    let commands: &[&[&str]] = &[
        &["inspect", "FIFO"],
        &["logo", "set", "FIFO"],
        &["rules", "set", "FIFO"],
        &["report-layout", "set", "terminals", "FIFO"],
        &[
            "import",
            "symbol-dxf",
            "FIFO",
            "--id",
            "t.dxf",
            "--name",
            "A",
            "--lib",
            "LIB",
        ],
        &["import", "footprint-dxf", "FIFO", "--part", "t.part", "--lib", "LIB"],
        &["import", "model-step", "FIFO", "--part", "t.part", "--lib", "LIB"],
        &[
            "import",
            "symbol-svg",
            "FIFO",
            "--id",
            "t.svg",
            "--name",
            "A",
            "--lib",
            "LIB",
        ],
        &["import", "plc-csv", "FIFO"],
        &["import", "wago", "FIFO"],
        &["import", "symbol-kicad", "FIFO", "--lib", "LIB"],
        &["import", "parts-bmecat", "FIFO", "--lib", "LIB"],
        &["import", "parts-edz", "FIFO", "--lib", "LIB"],
        &["import", "parts-edz", "FILE", "--lib", "LIB", "--symbol-map", "FIFO"],
        &["import", "symbol-qet", "FIFO", "FILE", "FIFO", "--lib", "LIB"],
        &["import", "parts-csv", "FIFO", "--lib", "LIB"],
        &["import", "archive", "FIFO"],
        &["import", "page-dwg", "FIFO", "--page", "p1"],
        &["import", "pdf", "FIFO"],
        &["import", "eplan", "FIFO"],
        &["import", "swe", "FIFO"],
        &["import", "swe", "FILE", "--symbol-map", "FIFO"],
        &["import", "swe-library", "FIFO"],
        &["import", "aml", "FIFO"],
        &["import", "page-dxf", "FIFO", "--page", "p1"],
    ];
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    new_project(&dir)?;
    let file = tmp.path().join("regular.txt");
    std::fs::write(&file, "not an input of any kind\n")?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(".wiringcraft.lock"))?;
    lock.lock()?;

    // All of them at once: each waits for the one lock held here.
    let mut running = Vec::new();
    for (n, args) in commands.iter().enumerate() {
        let mut command = Command::new(env!("CARGO_BIN_EXE_wiringcraft"));
        command.arg("-C").arg(&dir);
        let mut writers = Vec::new();
        for (k, arg) in args.iter().enumerate() {
            match *arg {
                "FIFO" => {
                    let fifo = tmp.path().join(format!("in-{n}-{k}.fifo"));
                    assert!(Command::new("mkfifo").arg(&fifo).status()?.success());
                    command.arg(&fifo);
                    let writer = {
                        let fifo = fifo.clone();
                        std::thread::spawn(move || {
                            std::fs::OpenOptions::new()
                                .write(true)
                                .open(&fifo)?
                                .write_all(b"not an input of any kind\n")
                        })
                    };
                    writers.push((fifo, writer));
                }
                "FILE" => {
                    command.arg(&file);
                }
                "LIB" => {
                    command.arg(tmp.path().join(format!("lib-{n}")));
                }
                other => {
                    command.arg(other);
                }
            }
        }
        let child = command.stdout(Stdio::null()).stderr(Stdio::piped()).spawn()?;
        running.push((args.join(" "), child, writers));
    }

    let start = Instant::now();
    let all_written = |running: &[(String, Child, Vec<FifoWriter>)]| {
        running
            .iter()
            .all(|(_, _, writers)| writers.iter().all(|(_, w)| w.is_finished()))
    };
    while !all_written(&running) && start.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let read_first: Vec<bool> = running
        .iter()
        .map(|(_, _, writers)| writers.iter().all(|(_, w)| w.is_finished()))
        .collect();
    std::thread::sleep(Duration::from_millis(300));
    let mut waits = Vec::new();
    for (_, child, _) in &mut running {
        waits.push(child.try_wait()?.is_none());
    }
    lock.unlock()?;

    let mut failures = Vec::new();
    for (((name, mut child, writers), read_first), waits) in running.into_iter().zip(read_first).zip(waits) {
        let ended = ends_within(&mut child, Duration::from_secs(30))?;
        if !ended {
            child.kill()?;
        }
        let out = child.wait_with_output()?;
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        let mut written = true;
        for (fifo, writer) in writers {
            written &= matches!(finish_writer(&fifo, writer)?, Some(Ok(())));
        }
        if !read_first {
            failures.push(format!("`{name}` did not read its FIFO before the lock: {err}"));
        }
        if !waits {
            failures.push(format!("`{name}` did not wait for the lock: {err}"));
        }
        if !ended {
            failures.push(format!("`{name}` did not end once the lock was free: {err}"));
        }
        if !written {
            failures.push(format!("the input of `{name}` was not written: {err}"));
        }
        if !err.contains("waiting for another wiringcraft command") {
            failures.push(format!("`{name}` did not say it waits: {err}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}
