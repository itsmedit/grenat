# frozen_string_literal: true

Gem::Specification.new do |spec|
  spec.name = "grenat-bridge"
  spec.version = "0.1.0"
  spec.summary = "Ruby functions a Grenat program calls: the server of a Grenat bridge facet"
  spec.description = "Exports Ruby functions to Grenat programs over JSON-RPC 2.0 on standard input and output. " \
                     "Standard library only."
  spec.authors = ["Mehdi Farsi"]
  spec.license = "MIT"
  spec.homepage = "https://github.com/itsmedit/grenat"
  spec.files = ["lib/grenat/bridge.rb"]
  spec.require_paths = ["lib"]
  spec.required_ruby_version = ">= 2.7"
end
