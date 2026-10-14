use super::*;

#[test]
fn marks_the_header_of_the_token_sensitive() {
    let token = Token {
        header: secret::Value::new(b"Token t0ken".to_vec()),
    };
    let header = token.header();
    assert!(header.is_sensitive());
    assert_eq!(header, "Token t0ken");
}
