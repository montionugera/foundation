use super::*;

#[test]
fn marks_the_header_of_the_token_sensitive() {
    let token = Token::new(&secret::Value::new(b"t0ken".to_vec())).expect("a token");
    let header = token.header();
    assert!(header.is_sensitive());
    assert_eq!(header, "Token t0ken");
}

#[test]
fn makes_no_token_of_a_value_with_a_control_byte() {
    assert!(Token::new(&secret::Value::new(b"t0\nken".to_vec())).is_none());
}
