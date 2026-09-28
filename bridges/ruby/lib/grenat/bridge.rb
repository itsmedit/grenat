# frozen_string_literal: true

require "json"

module Grenat
  # Ruby functions a Grenat program calls: the server of a bridge facet.
  #
  #   require "grenat/bridge"
  #
  #   Grenat::Bridge.struct(:Link, fields: {href: :string, text: :string}, doc: "A link of a page.")
  #
  #   Grenat::Bridge.export(:links, params: {html: :string}, returns: ["Link"], pure: true,
  #                         doc: "The links of a page.") do |html:|
  #     html.scan(/<a href="([^"]*)">([^<]*)</).map { |href, text| {href: href, text: text} }
  #   end
  #
  #   Grenat::Bridge.run
  #
  # Grenat starts the server (the `command` of the facet's `[bridge]`), and
  # speaks JSON-RPC 2.0 to it, a message per line: `describe` answers what
  # is exported (the manifest Grenat writes the facet's declarations from),
  # `call` runs a function with its arguments. While the server runs, what
  # the functions print goes to standard error, which Grenat logs: standard
  # output carries the protocol only.
  #
  # Types are written `:string`, `:int`, `:float`, `:bool`, `:nil`, `[t]`
  # (an array), `{string: t}` (a hash with string keys), or as Grenat
  # writes them: `"Link"`, `"String?"`.
  module Bridge
    # The version of the protocol (the manifest's `abi`).
    PROTOCOL = 1

    # An error a function raises to Grenat with a type of its choosing:
    # `raise Grenat::Bridge::Error.new("no such sheet", type: "SheetError")`.
    # Any other exception raises the function's `error:` type.
    class Error < StandardError
      attr_reader :type

      def initialize(message = nil, type: nil)
        super(message)
        @type = type
      end
    end

    # A request that is not JSON-RPC, or does not match what is exported.
    class ProtocolError < StandardError
      attr_reader :code

      def initialize(code, message)
        super(message)
        @code = code
      end
    end

    Function = Struct.new(:name, :params, :returns, :effects, :pure, :doc, :error, :block, keyword_init: true)

    SCALARS = {
      string: "String", int: "Int", integer: "Int", float: "Float",
      bool: "Bool", boolean: "Bool", nil: "Nil"
    }.freeze

    NAME = /\A[a-z_][a-z0-9_]*\z/.freeze

    @functions = {}
    @structs = []

    class << self
      # Exports the block as the Grenat function `name`: its parameters (in
      # order, the block takes them as keywords), result, effects (`"net"`,
      # `"fs.read"`), purity (a pure function has no effects, and its result
      # is trusted), documentation, and the Grenat error its exceptions raise.
      def export(name, params: {}, returns: :nil, effects: [], pure: false, doc: nil, error: "BridgeError", &block)
        name = name.to_s
        raise ArgumentError, "`#{name}` is not a Grenat function name" unless NAME.match?(name)
        raise ArgumentError, "`#{name}` needs a block: the function itself" unless block
        raise ArgumentError, "`#{name}` is pure, and has effects: a pure function has none" if pure && !effects.empty?

        params = params.to_h { |param, type| [param.to_sym, grenat_type(type)] }
        @functions[name] = Function.new(
          name: name, params: params, returns: grenat_type(returns), effects: effects.map(&:to_s),
          pure: pure, doc: doc, error: error.to_s, block: block
        )
        name.to_sym
      end

      # Declares a struct the functions take or return: a hash crosses as one.
      def struct(name, fields:, doc: nil)
        fields = fields.map { |field, type| {name: field.to_s, type: grenat_type(type), doc: nil} }
        @structs << {name: name.to_s, doc: doc, fields: fields}
        name.to_sym
      end

      # What is exported, as Grenat's manifest.
      def manifest
        functions = @functions.values.map do |f|
          {
            name: f.name, symbol: f.name, doc: f.doc,
            params: f.params.map { |param, type| {name: param.to_s, type: type, doc: nil} },
            returns: f.returns, effects: f.effects, pure: f.pure, error: f.error
          }
        end
        {abi: PROTOCOL, functions: functions, structs: @structs}
      end

      # The Grenat type written `spec`.
      def grenat_type(spec)
        case spec
        when Symbol
          SCALARS.fetch(spec) do
            return spec.to_s if spec.to_s.match?(/\A[A-Z]/)

            raise ArgumentError, "unknown type `#{spec.inspect}`: use #{SCALARS.keys.map(&:inspect).join(", ")}"
          end
        when String then spec
        when Array
          raise ArgumentError, "an array type has one element type: `[:string]`" unless spec.size == 1

          "Array(#{grenat_type(spec[0])})"
        when Hash
          key, value = spec.first
          raise ArgumentError, "a hash type has string keys: `{string: :int}`" unless spec.size == 1 && key == :string

          "Hash(String, #{grenat_type(value)})"
        else raise ArgumentError, "unknown type `#{spec.inspect}`"
        end
      end

      # The answer to one line of the protocol (nil for a notification).
      def handle(line)
        request = begin
          JSON.parse(line)
        rescue JSON::ParserError => e
          return failure(nil, ProtocolError.new(-32_700, "not JSON: #{e.message}"))
        end
        id = request.is_a?(Hash) ? request["id"] : nil
        begin
          raise ProtocolError.new(-32_600, "not a JSON-RPC 2.0 request") unless valid?(request)

          result = dispatch(request["method"], request["params"])
          request.key?("id") ? {jsonrpc: "2.0", id: id, result: result} : nil
        rescue ProtocolError, Error => e
          failure(id, e)
        end
      end

      # Serves requests until Grenat closes standard input.
      def run
        # the protocol is UTF-8 whatever the locale: a server often runs without one (US-ASCII then)
        Encoding.default_external = Encoding::UTF_8
        input = $stdin.dup.set_encoding(Encoding::UTF_8)
        output = $stdout.dup.set_encoding(Encoding::UTF_8)
        # standard output is the protocol's: what the functions print goes to standard error
        $stdin.reopen(File::NULL)
        $stdout.reopen($stderr)
        $stderr.sync = true
        output.sync = true
        input.each_line do |line|
          next if line.strip.empty?

          response = handle(line)
          output.write(encode(response), "\n") if response
        end
      end

      private

      def valid?(request)
        request.is_a?(Hash) && request["jsonrpc"] == "2.0" && request["method"].is_a?(String)
      end

      def dispatch(method, params)
        case method
        when "describe" then manifest
        when "call"
          raise ProtocolError.new(-32_602, "`call` takes {name, args}") unless params.is_a?(Hash)

          call(params["name"], params["args"])
        else raise ProtocolError.new(-32_601, "no method `#{method}`: `describe` or `call`")
        end
      end

      def call(name, args)
        function = @functions[name]
        raise ProtocolError.new(-32_602, "no function `#{name}` is exported") unless function
        unless args.is_a?(Array) && args.size == function.params.size
          raise ProtocolError.new(-32_602, "`#{name}` takes #{function.params.size} argument(s)")
        end

        begin
          function.params.empty? ? function.block.call : function.block.call(**function.params.keys.zip(args).to_h)
        rescue Error => e
          raise Error.new(e.message, type: e.type || function.error)
        rescue StandardError, ScriptError => e
          raise Error.new(e.message, type: function.error)
        end
      end

      def failure(id, error)
        code = error.is_a?(ProtocolError) ? error.code : -32_000
        body = {code: code, message: error.message}
        body[:data] = {type: error.type} if error.is_a?(Error)
        {jsonrpc: "2.0", id: id, error: body}
      end

      def encode(response)
        JSON.generate(response)
      rescue JSON::GeneratorError, Encoding::UndefinedConversionError => e
        JSON.generate(failure(response[:id], Error.new("the result cannot be sent as JSON: #{e.message}", type: "BridgeError")))
      end
    end
  end
end
