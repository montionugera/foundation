use super::*;

/// The size of the future that `send` returns.
fn size<'a, R>(
    _: impl FnOnce(
        &'a Client,
        &'a Context<Config>,
        &'a Uri,
        &'a RefCell<Option<Token>>,
        Bytes,
    ) -> R,
) -> usize {
    size_of::<R>()
}

#[test]
fn keeps_the_read_of_the_token_out_of_the_future_of_send() {
    let got = size(send);
    assert!(got <= 600, "the future of `send` is {got} B");
}

#[test]
fn marks_the_header_of_the_token_sensitive() {
    let token = Token::new(&secret::Value::new(b"t0ken".to_vec())).expect("a token");
    let uri = Uri::from_static("http://influx:8086/write");
    let request = request(&uri, &Bytes::new(), Some(&token));
    let header = &request.headers()[http::header::AUTHORIZATION];
    assert!(header.is_sensitive());
    assert_eq!(header, "Token t0ken");
}

#[test]
fn makes_no_token_of_a_value_with_a_control_byte() {
    assert!(Token::new(&secret::Value::new(b"t0\nken".to_vec())).is_none());
}
