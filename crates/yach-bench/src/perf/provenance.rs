use std::path::Path;
use std::process::Command;

use sha2::{Digest, Sha256};

use crate::perf::schema::{BuildInfo, HostInfo};

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn command_stdout(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("{program}: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("{program} failed: {stderr}"));
    }
    let stdout =
        String::from_utf8(output.stdout).map_err(|error| format!("{program} stdout: {error}"))?;
    Ok(stdout.trim().to_owned())
}

fn git_stdout(checkout: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|stdout| stdout.trim().to_owned())
        .filter(|stdout| !stdout.is_empty())
}

pub fn source_digest(checkout: &Path) -> Result<String, String> {
    let script = checkout.join("evals/scripts/source-digest.sh");
    let output = Command::new("bash")
        .arg(&script)
        .output()
        .map_err(|error| format!("source-digest.sh: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("source-digest.sh failed: {stderr}"));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| format!("source-digest.sh stdout: {error}"))?;
    Ok(stdout.trim().to_owned())
}

pub fn capture_build(checkout: &Path, yach_bin: Option<&Path>) -> Result<BuildInfo, String> {
    let source_sha256 = source_digest(checkout)?;
    let lock_bytes = std::fs::read(checkout.join("Cargo.lock"))
        .map_err(|error| format!("Cargo.lock: {error}"))?;
    let cargo_lock_sha256 = sha256_hex(&lock_bytes);
    let rustc = command_stdout("rustc", &["--version"])?;
    let commit = git_stdout(checkout, &["rev-parse", "HEAD"]);
    let porcelain = git_stdout(checkout, &["status", "--porcelain"]).unwrap_or_default();
    let dirty = commit.is_none() || !porcelain.is_empty();
    let yach_bin_sha256 = match yach_bin {
        Some(path) => {
            let bytes =
                std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
            Some(sha256_hex(&bytes))
        }
        None => None,
    };
    let profile = if cfg!(debug_assertions) {
        String::from("debug")
    } else {
        String::from("release")
    };
    Ok(BuildInfo {
        source_sha256,
        commit,
        dirty,
        profile,
        rustc,
        cargo_lock_sha256,
        yach_bin_sha256,
    })
}

fn cpu_model() -> String {
    #[cfg(target_os = "linux")]
    {
        if let Ok(contents) = std::fs::read_to_string("/proc/cpuinfo") {
            for line in contents.lines() {
                let Some(rest) = line.strip_prefix("model name") else {
                    continue;
                };
                if let Some((_, value)) = rest.split_once(':') {
                    let trimmed = value.trim();
                    if !trimmed.is_empty() {
                        return trimmed.to_owned();
                    }
                }
            }
        }
    }
    command_stdout("uname", &["-p"]).unwrap_or_else(|_| String::from("unknown"))
}

pub fn capture_host() -> HostInfo {
    let cpu = cpu_model();
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    let os = command_stdout("uname", &["-s"]).unwrap_or_else(|_| String::from("unknown"));
    let kernel = command_stdout("uname", &["-r"]).unwrap_or_else(|_| String::from("unknown"));
    let digest = sha256_hex(format!("{cpu}|{cores}|{os}|{kernel}").as_bytes());
    let fingerprint = digest.chars().take(16).collect();
    HostInfo {
        fingerprint,
        cpu,
        cores,
        os,
        kernel,
    }
}

#[cfg(test)]
mod tests {
    use super::{capture_build, capture_host};

    #[test]
    fn build_info_has_digest_and_lock_hash() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let captured = capture_build(&root, None);
        assert!(captured.is_ok(), "capture_build failed: {captured:?}");
        let Ok(build) = captured else { return };
        assert_eq!(build.source_sha256.len(), 64);
        assert_eq!(build.cargo_lock_sha256.len(), 64);
        assert!(build.rustc.contains("rustc"));
        assert!(build.commit.as_ref().is_none_or(|c| c.len() == 40));
    }

    #[test]
    fn host_fingerprint_is_stable_within_process() {
        assert_eq!(capture_host().fingerprint, capture_host().fingerprint);
        assert!(capture_host().cores > 0);
    }
}
