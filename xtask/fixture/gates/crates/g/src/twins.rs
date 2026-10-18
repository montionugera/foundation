#[cfg(target_os = "linux")]
mod a {
    #[test]
    fn x() {}
}

#[cfg(target_os = "macos")]
mod a {
    #[test]
    fn y() {}
}

#[cfg_attr(unix, test)]
fn attributed() {}
