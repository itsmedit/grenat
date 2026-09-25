//! `application/x-www-form-urlencoded`: form bodies and query strings.

/// `a=1&b=x%20y` → [(a, 1), (b, x y)].
pub fn parse(text: &str) -> Vec<(String, String)> {
    text.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (decode(k), decode(v)),
            None => (decode(p), String::new()),
        })
        .collect()
}

/// The first value of `name`.
pub fn get<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                match std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(b) => {
                        out.push(b);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms() {
        let pairs = parse("csrf=a%2Bb&token=x+y&flag&bad=%zz&end=%4");
        assert_eq!(get(&pairs, "csrf"), Some("a+b"));
        assert_eq!(get(&pairs, "token"), Some("x y"));
        assert_eq!(get(&pairs, "flag"), Some(""));
        assert_eq!(get(&pairs, "bad"), Some("%zz"));
        assert_eq!(get(&pairs, "end"), Some("%4"));
        assert_eq!(get(&pairs, "missing"), None);
    }
}
