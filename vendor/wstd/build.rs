use std::env;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(wstd_p2)");
    println!("cargo::rustc-check-cfg=cfg(wstd_p3)");

    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let wasip3 = env::var_os("CARGO_FEATURE_WASIP3").is_some() || target_env == "p3";
    let wasip2 = (env::var_os("CARGO_FEATURE_WASIP2").is_some() || target_env == "p2") && !wasip3;

    if wasip3 {
        println!("cargo::rustc-cfg=wstd_p3");
    } else if wasip2 {
        println!("cargo::rustc-cfg=wstd_p2");
    }
}
