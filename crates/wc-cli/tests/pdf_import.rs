//! `wiringcraft import pdf` and `import archive`, run as a program so the warnings are read
//! from stderr on their own, as a script that keeps the two streams apart reads them.

use std::io::Write;
use std::path::Path;
use std::process::Command;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A one-page PDF of `w` x `h` points with one line on it.
fn pdf(w: f64, h: f64) -> Vec<u8> {
    let content = "10 10 m 110 10 l S";
    format!(
        "%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 {w} {h}] >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n4 0 obj\n<< /Length {} >>\nstream\n{content}\nendstream\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n",
        content.len()
    )
    .into_bytes()
}

/// The program's stdout and stderr, each on its own; an error when it fails.
fn run(dir: &Path, args: &[&str]) -> Result<(String, String), Box<dyn std::error::Error>> {
    let out = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()?;
    let stderr = String::from_utf8(out.stderr)?;
    if !out.status.success() {
        return Err(stderr.into());
    }
    Ok((String::from_utf8(out.stdout)?, stderr))
}

fn project(at: &Path) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let dir = at.join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());
    Ok(dir)
}

// 3000 mm wide: beyond the largest custom sheet, so it is drawn on A0 with a warning.
const TOO_WIDE: (f64, f64) = (3000.0 / 25.4 * 72.0, 500.0 / 25.4 * 72.0);

#[test]
fn the_too_large_sheet_warning_names_the_pdf_it_came_from() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = project(tmp.path())?;
    let file = tmp.path().join("wide plan.pdf");
    std::fs::write(&file, pdf(TOO_WIDE.0, TOO_WIDE.1))?;
    let (_, stderr) = run(
        &dir,
        &["import", "pdf", file.to_str().ok_or("path")?, "--first-page", "200"],
    )?;
    assert!(stderr.contains("warning: page 200 (wide plan.pdf page 1)"), "{stderr}");
    assert!(stderr.contains("A0"), "{stderr}");
    // And the page is drawn on A0.
    let project = wc_core::store::load(&dir)?;
    let page = project.page("pdf-200").ok_or("no page pdf-200")?;
    assert_eq!(page.size, wc_core::PageSize::A0);
    Ok(())
}

#[test]
fn in_an_archive_the_warning_names_the_entry_with_control_characters_escaped() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = project(tmp.path())?;
    let archive = tmp.path().join("project.zip");
    {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive)?);
        let opts = zip::write::SimpleFileOptions::default();
        // "a-small.pdf" sorts before "plans/...", so the wide one comes second, after one page.
        zip.start_file("a-small.pdf", opts)?;
        zip.write_all(&pdf(842.0, 595.0))?;
        zip.start_file("plans/wide\u{1b}[2J.pdf", opts)?;
        zip.write_all(&pdf(TOO_WIDE.0, TOO_WIDE.1))?;
        zip.finish()?;
    }
    let (stdout, stderr) = run(&dir, &["import", "archive", archive.to_str().ok_or("path")?])?;
    let warning = stderr
        .lines()
        .find(|l| l.contains("A0"))
        .ok_or_else(|| format!("no warning in {stderr:?}"))?;
    // Named by its entry and not by the staging file it was read from.
    assert!(warning.starts_with("warning: page 301 "), "{warning}");
    assert!(warning.contains(" (plans/wide\\u{1b}[2J.pdf page 1): "), "{warning}");
    assert!(
        !stderr.contains('\u{1b}') && !stdout.contains('\u{1b}'),
        "{stdout:?} {stderr:?}"
    );
    Ok(())
}
