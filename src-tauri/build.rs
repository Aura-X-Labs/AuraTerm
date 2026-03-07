use std::process::Command;

fn main() {
    // 获取构建时间
    let build_time = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    println!("cargo:rustc-env=BUILD_TIME={}", build_time);
    
    // 获取 Git commit hash (short version)
    let git_hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout).ok()
            } else {
                None
            }
        })
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    
    println!("cargo:rustc-env=GIT_COMMIT={}", git_hash);
    
    // 监听 git 相关文件变化，触发重新构建
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads/");
    
    tauri_build::build()
}