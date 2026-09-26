#!/bin/sh
# The Homebrew formula of a published release, for the tap
# (github.com/itsmedit/homebrew-grenat, Formula/grenat.rb):
#
#   scripts/homebrew-formula.sh v0.1.0 > ../homebrew-grenat/Formula/grenat.rb
set -eu
tag="$1"
base="https://github.com/itsmedit/grenat/releases/download/$tag"
sha() {
    curl -fsSL "$base/grenat-$tag-$1.tar.gz.sha256" | cut -d ' ' -f 1
}
target() {
    printf '      url "%s/grenat-%s-%s.tar.gz"\n      sha256 "%s"\n' "$base" "$tag" "$1" "$(sha "$1")"
}
cat <<RUBY
class Grenat < Formula
  desc "Agentic programming language: Ruby's syntax, Rust's speed"
  homepage "https://github.com/itsmedit/grenat"
  version "${tag#v}"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
$(target aarch64-apple-darwin)
    end
    on_intel do
$(target x86_64-apple-darwin)
    end
  end

  on_linux do
    on_arm do
$(target aarch64-unknown-linux-gnu)
    end
    on_intel do
$(target x86_64-unknown-linux-gnu)
    end
  end

  def install
    bin.install "bin/grenat", "bin/setter"
    # the runtime libraries \`grenat build\` links, found next to the binary
    (lib/"grenat").install Dir["lib/grenat/*.a"]
  end

  test do
    (testpath/"hello.grn").write "def main\\n  puts \\"hello\\"\\nend\\n"
    assert_equal "hello\\n", shell_output("#{bin}/grenat run #{testpath}/hello.grn")
    system bin/"grenat", "build", "--native", testpath/"hello.grn", "-o", testpath/"hello"
    assert_equal "hello\\n", shell_output(testpath/"hello")
  end
end
RUBY
