# frozen_string_literal: true

$LOAD_PATH.unshift File.expand_path("../lib", __dir__)

require "minitest/autorun"
require "socket"
require "json"
require "vunly"

# A synthetic key: it embeds the marker `synthetic` so the repository's secret audit allowlists it.
SYNTHETIC_KEY = "vk_test_synthetic_ruby_sdk_0000000001"

# A scriptable HTTP/1.1 server on a local port. One request per connection (the SDK sends Connection: close). The block
# gets (socket, request) and writes whatever it likes, including a broken or dripping answer.
class FakeServer
  Req = Struct.new(:verb, :path, :headers, :body, :n)

  attr_reader :port

  def initialize(&handler)
    @handler = handler
    @requests = []
    @mutex = Mutex.new
    @server = TCPServer.new("127.0.0.1", 0)
    @port = @server.addr[1]
    @connections = []
    @thread = Thread.new { accept_loop }
    @thread.report_on_exception = false
  end

  # The program-friendly way: a list of steps, one per request (the last repeats).
  def self.script(*steps)
    new do |io, req|
      step = steps[[req.n - 1, steps.size - 1].min]
      step.call(io, req)
    end
  end

  def url
    "http://127.0.0.1:#{port}"
  end

  def requests
    @mutex.synchronize { @requests.dup }
  end

  def hits
    requests.size
  end

  def stop
    @server.close
    @mutex.synchronize { @connections.dup }.each(&:kill)
    @thread.kill
  end

  # ---- answers a handler can give --------------------------------------------------------------------------------

  def self.json(status, body = {}, headers = {})
    lambda do |io, _req|
      payload = body.is_a?(String) ? body : JSON.generate(body)
      head = { "Content-Type" => "application/json", "Content-Length" => payload.bytesize.to_s,
               "Connection" => "close" }.merge(headers)
      io.write("HTTP/1.1 #{status} X\r\n#{head.map { |k, v| "#{k}: #{v}\r\n" }.join}\r\n#{payload}")
    end
  end

  def self.error(status, code, message = "x", details = nil, headers = {})
    json(status, { error: { code: code, message: message, requestId: "req_test", details: details }.compact }, headers)
  end

  # reads the request and closes without answering
  def self.drop
    ->(io, _req) { io.close }
  end

  # ---- internals -------------------------------------------------------------------------------------------------

  private

  def accept_loop
    loop do
      client = @server.accept
      t = Thread.new(client) { |io| serve(io) }
      t.report_on_exception = false
      @mutex.synchronize { @connections << t }
    end
  rescue IOError, Errno::EBADF
    nil
  end

  def serve(io)
    request = read_request(io)
    return unless request

    @handler.call(io, request)
  rescue Errno::EPIPE, Errno::ECONNRESET, IOError
    nil
  ensure
    begin
      io.close unless io.closed?
    rescue StandardError
      nil
    end
  end

  def read_request(io)
    line = io.gets("\r\n")
    return nil unless line

    verb, path, = line.split
    headers = {}
    while (h = io.gets("\r\n")) && h != "\r\n"
      k, v = h.chomp.split(": ", 2)
      headers[k.downcase] = v
    end
    body = (n = headers["content-length"].to_i).positive? ? io.read(n) : ""
    @mutex.synchronize do
      Req.new(verb, path, headers, body, @requests.size + 1).tap { |r| @requests << r }
    end
  end
end

module TestSupport
  def new_client(server, **)
    sleeps = []
    client = Vunly::Client.new(api_key: SYNTHETIC_KEY, base_url: server.url,
                               sleeper: ->(s) { sleeps << s }, **)
    [client, sleeps]
  end

  # Runs the block with a server that answers with the given steps, one per request (the last repeats).
  def with_server(*steps)
    server = FakeServer.script(*steps)
    yield server
  ensure
    server&.stop
  end

  # The same with a handler of your own: a lambda(io, request).
  def with_handler(handler)
    server = FakeServer.new(&handler)
    yield server
  ensure
    server&.stop
  end

  def customer_json(extra = {})
    { id: "cus_1", externalId: "acme", createdAt: "2026-10-28T10:00:00.000Z" }.merge(extra)
  end

  def entry_json(extra = {})
    { id: "led_1", customerId: "cus_1", kind: "debit", credits: -3, meter: "tokens", quantity: 3000,
      idempotencyKey: "use-0001-abcd", createdAt: "2026-10-28T10:00:00.000Z" }.merge(extra)
  end

  def usage_json
    { event: entry_json, balance: 97 }
  end

  def plan_json(extra = {})
    { key: "starter", name: "Starter", includedCredits: 100, meter: "tokens", features: ["generate"] }.merge(extra)
  end

  def subscription_json
    { id: "sub_1", customerId: "cus_1", status: "active", plan: plan_json,
      periodStart: "2026-10-01T00:00:00.000Z", periodEnd: "2026-11-01T00:00:00.000Z",
      createdAt: "2026-10-01T00:00:00.000Z" }
  end

  def plan_usage_json
    { plan: { key: "free" }, period: { start: "2026-10-01T00:00:00.000Z", end: "2026-11-01T00:00:00.000Z" },
      usage: { events: 4 },
      unconfirmedLimits: { confirmed: false, note: "n", includedMonthlyEvents: 1000, activityRetentionDays: nil } }
  end

  def token_json
    { id: "tok_1", token: "vct_synthetic_x", customerId: "cus_1", expiresAt: "2026-10-28T10:15:00.000Z" }
  end
end
