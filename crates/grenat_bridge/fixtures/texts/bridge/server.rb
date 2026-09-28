# The Ruby side of the facet `texts`: functions of every kind, errors, a
# process that dies or sleeps, and one that tries the network it was not given.

require "socket"
require "grenat/bridge"

Bridge = Grenat::Bridge

Bridge.struct(:Word, fields: {text: :string, position: :int}, doc: "A word of a text.")

Bridge.export(:shout, params: {text: :string}, returns: :string, doc: "The text, in capitals.") { |text:| text.upcase }

Bridge.export(:add, params: {a: :int, b: :int}, returns: :int, pure: true, doc: "Adds two integers.") do |a:, b:|
  a + b
end

Bridge.export(:words, params: {text: :string}, returns: ["Word"], pure: true) do |text:|
  text.split.each_with_index.map { |word, i| {text: word, position: i} }
end

Bridge.export(:first_word, params: {text: :string}, returns: "String?", pure: true) { |text:| text.split.first }

Bridge.export(:count, params: {text: :string}, returns: {string: :int}, pure: true) { |text:| text.split.tally }

Bridge.export(:read_text, params: {path: :string}, returns: :string, effects: ["fs.read"], error: "TextError") do |path:|
  File.read(path)
end

Bridge.export(:refuse, params: {reason: :string}) do |reason:|
  raise Grenat::Bridge::Error.new(reason, type: "RefusalError")
end

Bridge.export(:greeting, returns: "String?") { ENV["TEXTS_GREETING"] }

Bridge.export(:variable, params: {name: :string}, returns: "String?") { |name:| ENV[name] }

Bridge.export(:chatter, params: {text: :string}, returns: :string) do |text:|
  puts "printed #{text}"
  warn "warned #{text}"
  text
end

Bridge.export(:die, params: {status: :int}) do |status:|
  warn "dying"
  exit!(status)
end

Bridge.export(:nap, params: {seconds: :float}) { |seconds:| sleep(seconds) }

Bridge.export(:pid, returns: :int) { Process.pid }

# dies the first time it is called: a pure function is sent again to a new process
Bridge.export(:flaky, params: {marker: :string}, returns: :string, pure: true) do |marker:|
  next "recovered" if File.exist?(marker)

  File.write(marker, "")
  exit!(1)
end

Bridge.export(:connect, params: {port: :int}, returns: :bool) do |port:|
  TCPSocket.new("127.0.0.1", port).close
  true
rescue SystemCallError
  false
end

Bridge.run
