fn main() {
    println!("cargo:rustc-link-arg-bins=-Tlinkall.x");

    println!("cargo:rustc-link-arg-bins=-Trom_functions.x");

    // load environment variables from .env file
    use dotenvy::{dotenv, dotenv_iter};
    let dotenv_path = dotenv().expect("Failed to find .env file");
    println!("cargo:rerun-if-changed={}", dotenv_path.display());

    for item in dotenv_iter().unwrap() {
        let (key, value) = item.unwrap();
        println!("cargo:rustc-env={key}={value}");
    }
}
