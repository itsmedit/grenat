//! PDF documents and images in prompts.

mod common;

use common::*;
use grenat_interp::Response;
use serde_json::json;

const PROMPT: &str = "\
model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"
prompt read(doc: Attachment, question: String) -> ~String using :fast
  user \"Answer from the document.\", doc
  user question
end
";

fn request(src: &str) -> serde_json::Value {
    let r = run_with(src, vec![Response::text_reply("ok")], &[]);
    let requests = r.requests.clone();
    r.ok();
    requests[0]["messages"].clone()
}

#[test]
fn a_pdf_goes_first_as_a_document_block() {
    let dir = temp_dir("attachments");
    std::fs::write(dir.join("invoice.pdf"), b"%PDF-1.4 x").unwrap();
    let messages = request(&format!("{PROMPT}read(Pdf.read(\"{}/invoice.pdf\"), \"Total?\")\n", dir.display()));
    assert_eq!(messages.as_array().unwrap().len(), 1);
    assert_eq!(
        messages[0]["content"],
        json!([
            {"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": "JVBERi0xLjQgeA=="}},
            {"type": "text", "text": "Answer from the document.\n\nTotal?"},
        ])
    );
}

#[test]
fn images_and_urls() {
    let dir = temp_dir("images");
    std::fs::write(dir.join("scan.PNG"), [137u8, 80, 78, 71]).unwrap();
    let messages = request(&format!("{PROMPT}read(Image.read(\"{}/scan.PNG\"), \"What?\")\n", dir.display()));
    assert_eq!(messages[0]["content"][0]["source"]["media_type"], "image/png");
    assert_eq!(messages[0]["content"][0]["type"], "image");
    let messages = request(&format!("{PROMPT}read(Pdf.url(\"https://x.io/a.pdf\"), \"What?\")\n"));
    assert_eq!(messages[0]["content"][0], json!({"type": "document", "source": {"type": "url", "url": "https://x.io/a.pdf"}}));
}

#[test]
fn text_only_messages_are_unchanged() {
    let src = "model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"\nprompt t(x: String) -> ~String using :fast\n  user \"a\", x\n  user \"b\"\nend\nt(\"z\")\n";
    assert_eq!(request(src)[0]["content"], "a\nz\n\nb");
}

#[test]
fn reading_is_a_capability_and_formats_are_checked() {
    let dir = temp_dir("formats");
    std::fs::write(dir.join("notes.txt"), "x").unwrap();
    let e = run_err(&format!("Image.read(\"{}/notes.txt\")\n", dir.display()), Vec::new());
    assert!(e.message.ends_with("images are .png, .jpg, .gif or .webp"), "{}", e.message);
    let e = run_err("def f uses fs.read(\"./docs\")\n  Pdf.read(\"/etc/hosts.pdf\")\nend\nf\n", Vec::new());
    assert_eq!(e.ty, "CapabilityError");
    let e = run_err("Pdf.url(\"file:///etc/passwd\")\n", Vec::new());
    assert_eq!(e.ty, "ArgumentError");
}
