#[demo_macros::collect]
pub mod handlers_a {
    #[registry::handler("greet")]
    pub fn greet(input: &str) -> String {
        format!("hello, {input}")
    }
}
