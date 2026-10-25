#[demo_macros::collect]
pub mod handlers_b {
    #[registry::handler("greet")]
    pub fn greet_again(input: &str) -> String {
        format!("hi, {input}")
    }
}
