use revlens::config::Config;

#[test]
fn example_config_equals_defaults() {
    let text = include_str!("../examples/config.toml");
    assert_eq!(
        Config::parse(text).expect("parse example"),
        Config::default()
    );
}
