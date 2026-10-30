# frozen_string_literal: true

require "net/http"

module Vunly
  # The HTTP layer: one {Net::HTTP} connection per attempt, retries by operation class, and the limits that conformance/README.md
  # asks for.
  #
  # - One attempt is bounded as a WHOLE (connect, headers and body) by a monotonic deadline that a watchdog thread
  #   enforces by closing the connection, so a server that answers at once and then drips its body (or its headers) is cut.
  #   No +Timeout.timeout+ is used.
  # - The body is read in chunks and capped at 16 MB while streaming.
  # - +Net::HTTP+ never retries or replays: +max_retries = 0+, and every attempt has its own connection.
  # - Redirects are not followed and no proxy variable is read.
  #
  # @api private
  class Transport
    include Undumpable

    # The most a response body may be (bytes), checked while it streams.
    MAX_BODY = 16 * 1024 * 1024
    # The longest +Retry-After+ the SDK waits for (seconds); beyond it the error is raised instead.
    MAX_RETRY_AFTER = 30
    # Bounds on what a server can make appear in an error (characters), and on the JSON the SDK is willing to parse.
    MAX_MESSAGE = 500
    MAX_REQUEST_ID = 128
    MAX_CODE = 64
    MAX_NESTING = 32
    # JSON pre-scan limits: the API's pages hold at most 100 items of a dozen fields, so a body with more than this many
    # arrays and objects, or commas (elements), is not one it sends, whatever its size under the 16 MB cap. Counting is a
    # byte scan; nothing is allocated for a body that fails it.
    MAX_CONTAINERS = 20_000
    MAX_ELEMENTS = 200_000
    # A run of this many digits is never a number the API sends (credits fit in 64 bits).
    HUGE_NUMBER = /[0-9]{1000}/
    # Control characters (C0, DEL, C1), format characters (BiDi marks and overrides, zero-width, BOM) and line separators.
    UNSAFE_TEXT = /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/
    # What an idempotency id may look like. +\A+ and +\z+, not +^+ and +$+: Ruby's anchors match at every line.
    ID_PATTERN = /\A[A-Za-z0-9_.:-]{8,128}\z/
    # Headers kept from a response (lower case).
    KEPT_HEADERS = %w[request-id retry-after idempotent-replayed vunly-plan-overage content-length].freeze
    # A request the transport sends.
    Request = Data.define(:verb, :path, :query, :body, :idempotency_key, :operation_class, :shape)
    # What came back from one attempt.
    Raw = Struct.new(:status, :headers, :body, :too_large, :failure, :complete)
    # What the SDK reads from the headers of a success.
    Meta = Data.define(:status, :request_id, :replayed, :plan_overage)

    # Raised inside an attempt when its deadline has passed.
    class Expired < StandardError; end

    # Closes the connection of an attempt when the attempt's whole time is up, from a thread of its own.
    class Watchdog
      def initialize(http, seconds)
        @http = http
        @deadline = Transport.now + seconds
        @mutex = Mutex.new
        @cond = ConditionVariable.new
        @state = :armed
        @thread = Thread.new { watch }
        @thread.report_on_exception = false
      end

      def remaining
        @deadline - Transport.now
      end

      def expired?
        @state == :expired
      end

      # Raises {Expired} when time is up.
      def check!
        raise Expired if @state == :expired || remaining <= 0
      end

      def stop
        @mutex.synchronize do
          @state = :done if @state == :armed
          @cond.signal
        end
        @thread.join
      end

      private

      def watch
        fire = @mutex.synchronize do
          while @state == :armed && (left = remaining).positive?
            @cond.wait(@mutex, left)
          end
          @state == :armed && (@state = :expired)
        end
        close if fire
      end

      def close
        @http.finish
      rescue StandardError
        nil # not connected yet (the connect phase has its own bound) or already closed
      end
    end

    # @return [String] the API address without a trailing slash or +/v1+
    attr_reader :base_url

    # @param api_key [Secret]
    # @param base_url [String] already normalised
    # @param timeout [Numeric] seconds for ONE attempt, as a whole
    # @param max_retries [Integer]
    # @param sleeper [#call] waits a number of seconds
    # @param user_agent [String]
    def initialize(api_key:, base_url:, timeout:, max_retries:, sleeper:, user_agent:)
      @api_key = api_key
      @base_url = base_url
      uri = URI.parse(base_url)
      @host = uri.hostname
      @port = uri.port
      @tls = uri.scheme == "https"
      @prefix = uri.path.to_s
      @timeout = timeout.to_f
      @max_retries = max_retries
      @sleeper = sleeper
      @user_agent = user_agent
    end

    # @return [Float] seconds on a clock that never goes back
    def self.now
      Process.clock_gettime(Process::CLOCK_MONOTONIC)
    end

    # Makes text a server supplied safe to put in an exception message or a log line: valid UTF-8, no control, BiDi or
    # zero-width characters, at most +max+ characters.
    #
    # @api private
    # @return [String, nil] nil when nothing printable is left
    def self.clean(value, max)
      return nil unless value.is_a?(String)

      text = value.dup.force_encoding(Encoding::UTF_8).scrub("?").gsub(UNSAFE_TEXT, "").strip
      text = text[0, max] if text.length > max
      text.empty? ? nil : text
    end

    # A copy of the API's +details+ that is safe to keep and print: at most 5 levels, 50 entries per object or array, keys
    # cleaned to 64 characters and strings to 500, nothing but JSON scalars.
    #
    # @api private
    def self.clean_details(value, depth = 0)
      case value
      when Hash
        return nil if depth > 4

        value.first(50).each_with_object({}) do |(key, item), out|
          name = clean(key, MAX_CODE)
          out[name] = clean_details(item, depth + 1) if name
        end
      when Array then depth > 4 ? nil : value.first(50).map { |item| clean_details(item, depth + 1) }
      when String then clean(value, MAX_MESSAGE) || ""
      when Integer, Float, true, false, nil then value
      end
    end

    # The error for a 2xx that cannot be used. Unavailable whatever the status: the call may have been applied, so
    # retrying later with the SAME id is safe.
    #
    # @return [UnavailableError]
    def self.invalid_response(status, request_id, what)
      UnavailableError.new("the API answered #{status} but the response is malformed (#{what})",
                           code: "invalid_response", status: status, request_id: request_id)
    end

    # Normalises and checks an API address; the message never repeats it (it may hold credentials).
    #
    # @param url [String, nil]
    # @return [String]
    # @raise [ConfigurationError]
    def self.normalize_base_url(url)
      text = url.to_s.strip
      raise ConfigurationError, "base_url is empty (use the API address from the setup step)" if text.empty?

      out = text.sub(%r{/+\z}, "").sub(%r{/v1\z}i, "").sub(%r{/+\z}, "")
      begin
        uri = URI.parse(out)
      rescue URI::InvalidURIError
        raise ConfigurationError, "base_url must be an absolute http(s) address such as https://host", cause: nil
      end
      unless uri.is_a?(URI::HTTP) && uri.host && !uri.host.empty? # URI::HTTPS is a URI::HTTP
        raise ConfigurationError, "base_url must be an absolute http(s) address such as https://host"
      end
      # any "@" in the authority (even an empty userinfo, "http://@host") is credentials syntax: refused
      if uri.userinfo || uri.query || uri.fragment || out.match?(%r{\A[A-Za-z][A-Za-z0-9+.-]*://[^/?#]*@})
        raise ConfigurationError, "base_url must not contain credentials, a query or a fragment"
      end
      raise ConfigurationError, "base_url has an invalid port" unless (1..65_535).cover?(uri.port)

      out
    end

    # Sends a request with the retries its class allows.
    #
    # @param req [Request]
    # @return [Array(Object, Meta)] the built result (or nil) and the response metadata
    # @raise [Error]
    def call(req)
      # a keyed mutation MUST have an id, and a good one: no random default, nothing sent without it
      check_id!(req.idempotency_key) if req.idempotency_key || req.operation_class == :k
      payload = encode(req.body)
      attempt = 1
      loop do
        return once(req, payload)
      rescue UnavailableError => e
        raise unless attempt <= @max_retries && may_retry?(req.operation_class, e)

        wait = retry_delay(attempt, e.retry_after_seconds)
        raise if wait.nil?

        @sleeper.call(wait)
        attempt += 1
      end
    end

    # Refuses, before any request, an id that could not be an Idempotency-Key: the same error the API would give.
    #
    # @param id [Object]
    # @raise [InvalidRequestError]
    def check_id!(id)
      return if id.is_a?(String) && ID_PATTERN.match?(id)

      raise InvalidRequestError.new(
        "id must be 8 to 128 characters of A-Za-z0-9_.:- (it is sent as the Idempotency-Key)",
        code: "validation_error", status: 400, details: { "fields" => { "id" => "8-128 chars of A-Za-z0-9_.:-" } }
      )
    end

    # The longest +retry_after_seconds+ an error reports (a day); a larger or unrepresentable wait is reported as this.
    REPORT_CAP = 86_400.0
    RETRY_AFTER_NUMBER = /\A\+?(?:([0-9]+)(?:\.([0-9]*))?|\.([0-9]+))(?:[eE]([+-]?[0-9]+))?\z/
    RETRY_AFTER_INFINITE = /\A\+?inf(?:inity)?\z/i
    private_constant :RETRY_AFTER_NUMBER, :RETRY_AFTER_INFINITE

    # Reads a +Retry-After+ per conformance/README.md: an optional +, digits, an optional fraction, an
    # optional exponent, or Inf/Infinity; nothing else. Nothing is parsed with +Float()+ or +to_f+ (hex, suffixes,
    # Infinity on overflow); the value is exact (Rational) and capped at a day.
    #
    # @api private
    # @param header [String]
    # @return [Float, nil] seconds (at most 86400), or nil when the header is not such a number (treated as absent)
    def self.parse_retry_after(header)
      text = header.strip
      return REPORT_CAP if RETRY_AFTER_INFINITE.match?(text)

      match = RETRY_AFTER_NUMBER.match(text) or return nil
      fraction = match[2] || match[3] || "" # "5." and ".5" are decimals; a lone "." did not match
      mantissa = Rational("#{match[1]}#{fraction}".to_i, 10**fraction.length)
      return 0.0 if mantissa.zero?

      exponent = match[4].to_s
      if exponent.delete_prefix("+").delete_prefix("-").size > 6 # an absurd exponent: a very long wait, or none at all
        return exponent.start_with?("-") ? 0.0 : REPORT_CAP
      end

      seconds = mantissa * (Rational(10)**exponent.to_i.clamp(-400, 400))
      seconds > REPORT_CAP ? REPORT_CAP : seconds.to_f
    end

    private

    def encode(body)
      return nil if body.nil?

      JSON.generate(body)
    rescue JSON::GeneratorError, EncodingError
      raise ArgumentError, "the request cannot be encoded as JSON", cause: nil
    end

    # R, K and N are repeated after a lost answer, a timeout or a 5xx; X only after a 429, which was refused before it
    # ran. The plan-volume stop is never repeated. A 2xx (invalid_response) is not a retryable status.
    def may_retry?(operation_class, error)
      return false unless [0, 429, 500, 502, 503, 504].include?(error.status)
      return false if error.plan_stop?
      return false if operation_class == :x && error.status != 429

      true
    end

    def retry_delay(attempt, retry_after)
      if retry_after&.positive?
        return retry_after <= MAX_RETRY_AFTER ? retry_after : nil
      end

      [0.25 * (2**(attempt - 1)), 8.0].min * (0.5 + (rand / 2))
    end

    def once(req, payload)
      raw = exchange(req, payload)
      meta = Meta.new(status: raw.status, request_id: raw.headers["request-id"],
                      replayed: raw.headers["idempotent-replayed"] == "true",
                      plan_overage: raw.headers["vunly-plan-overage"] == "true")
      raise error_from_response(raw) unless raw.status.between?(200, 299)
      raise lost("the connection broke while the answer was being read", raw.failure) if raw.failure
      raise invalid_response(raw, "the body is larger than #{MAX_BODY / (1024 * 1024)} MB") if raw.too_large

      [decode(raw, req), meta]
    end

    # ---- one attempt -------------------------------------------------------------------------------------------------

    def new_http
      http = BoundedHTTP.new(@host, @port, nil) # nil: no proxy, and the proxy environment variables are not read
      http.use_ssl = @tls
      http.open_timeout = http.read_timeout = http.write_timeout = @timeout
      http.max_retries = 0 # Net::HTTP would repeat an idempotent request once by itself; only this SDK retries
      http.ignore_eof = false # a body shorter than its Content-Length is an error, not a short body
      http
    end

    def exchange(req, payload)
      http = new_http
      request = build_request(req, payload)
      watchdog = Watchdog.new(http, @timeout)
      raw = nil
      failure = nil
      begin
        catch(:vunly_abort) do
          http.start do |h|
            watchdog.check!
            bound(h, watchdog)
            h.request(request) do |res|
              raw = start_raw(res)
              read(h, res, watchdog, raw)
            end
          end
        end
      rescue StandardError
        failure = watchdog.expired? || watchdog.remaining <= 0 ? :timeout : :broken
      ensure
        watchdog.stop
      end
      conclude(raw, failure)
    end

    def conclude(raw, failure)
      raise lost("could not reach the Vunly API", failure || :broken) if raw.nil?

      # an error after the whole body was read (while the connection was being closed) does not spoil the answer
      raw.failure ||= failure if failure && !raw.complete
      raw
    end

    def lost(message, failure)
      message = "the Vunly API did not answer within #{format('%g', @timeout)} s" if failure == :timeout
      UnavailableError.new(message, code: "unavailable", status: 0)
    end

    def build_request(req, payload)
      path = @prefix + req.path
      path += "?#{URI.encode_www_form(req.query)}" if req.query && !req.query.empty?
      headers = {
        "Authorization" => "Bearer #{@api_key.reveal}",
        "User-Agent" => @user_agent,
        "Accept" => "application/json",
        # Net::HTTP would add gzip and inflate it; this API's answers are small, and a plain body keeps the size cap exact
        "Accept-Encoding" => "identity",
        "Connection" => "close"
      }
      headers["Idempotency-Key"] = req.idempotency_key if req.idempotency_key
      headers["Content-Type"] = "application/json" if payload
      klass = { "GET" => Net::HTTP::Get, "POST" => Net::HTTP::Post, "DELETE" => Net::HTTP::Delete }.fetch(req.verb)
      klass.new(path, headers).tap { |r| r.body = payload if payload }
    end

    # Whatever time is left is all a single read or write may take.
    def bound(http, watchdog)
      left = [watchdog.remaining, 0.001].max
      http.read_timeout = left
      http.write_timeout = left
    end

    def start_raw(res)
      headers = {}
      res.each_header { |k, v| headers[k.downcase] = v if KEPT_HEADERS.include?(k.downcase) }
      headers["request-id"] = Transport.clean(headers["request-id"], MAX_REQUEST_ID)
      Raw.new(res.code.to_i, headers, +"", false, nil, false)
    end

    # Reads the body into +raw+ in chunks, under the attempt's deadline and the size cap. Whenever it stops early it
    # THROWS out of the request block: returning normally would let Net::HTTP read "the rest of the body" itself,
    # without either limit.
    def read(http, res, watchdog, raw)
      declared = Integer(raw.headers["content-length"], exception: false)
      abort_read(raw, too_large: true) if declared && declared > MAX_BODY
      buffer = String.new(encoding: Encoding::BINARY)
      begin
        res.read_body do |chunk|
          watchdog.check!
          bound(http, watchdog)
          abort_read(raw, too_large: true) if buffer.bytesize + chunk.bytesize > MAX_BODY
          buffer << chunk
        end
      rescue StandardError
        abort_read(raw, failure: watchdog.expired? || watchdog.remaining <= 0 ? :timeout : :broken)
      end
      abort_read(raw, failure: :broken) if declared && declared != buffer.bytesize
      raw.body = buffer
      raw.complete = true
    end

    def abort_read(raw, too_large: false, failure: nil)
      raw.too_large = too_large
      raw.failure = failure
      raw.complete = too_large
      throw :vunly_abort
    end

    # ---- answers -----------------------------------------------------------------------------------------------------

    def decode(raw, req)
      return nil if req.shape.nil?

      parsed = parse_json(raw.body)
      raise invalid_response(raw, "the body is not a JSON object") unless parsed.is_a?(Hash)

      problem = shape_problem(parsed, req.shape)
      raise invalid_response(raw, problem) if problem

      parsed
    end

    # Parses a body that already passed the size cap, with bounds of its own: at most MAX_NESTING levels and MAX_CONTAINERS
    # arrays and objects (a 16 MB body of "[],[],..." would otherwise allocate millions of objects). Anything over is not
    # JSON this API sends: nil, and the caller makes it invalid_response.
    def parse_json(body)
      text = body.dup.force_encoding(Encoding::UTF_8)
      return nil if text.count("[{") > MAX_CONTAINERS || text.count(",") > MAX_ELEMENTS
      return nil if text.match?(HUGE_NUMBER) # converting a million-digit integer takes seconds and memory

      JSON.parse(text, max_nesting: MAX_NESTING)
    rescue JSON::ParserError, EncodingError, ArgumentError
      nil
    end

    def invalid_response(raw, what)
      Transport.invalid_response(raw.status, raw.headers["request-id"], what)
    end

    def error_from_response(raw)
      return redirect_error(raw) if raw.status.between?(300, 399)

      parsed = raw.too_large || raw.failure ? nil : parse_json(raw.body)
      err = parsed.is_a?(Hash) && parsed["error"].is_a?(Hash) ? parsed["error"] : {}
      code = Transport.clean(err["code"], MAX_CODE) || fallback_code(raw.status)
      request_id = Transport.clean(err["requestId"], MAX_REQUEST_ID) || raw.headers["request-id"]
      details = err["details"].is_a?(Hash) ? Transport.clean_details(err["details"]) : nil
      Error.for_status(raw.status, code, error_message(raw, err), request_id: request_id, details: details,
                                                                  retry_after_seconds: retry_after(raw))
    end

    # A redirect is never followed (the key must not leave the configured host) and is not an answer to retry.
    def redirect_error(raw)
      UnavailableError.new("the API answered a redirect (#{raw.status}), which the SDK never follows; " \
                           "check the API address you configured",
                           code: "unexpected_redirect", status: raw.status, request_id: raw.headers["request-id"])
    end

    def error_message(raw, err)
      message = Transport.clean(err["message"], MAX_MESSAGE) || "request failed with status #{raw.status}"
      return message unless raw.status == 404 && message.downcase.include?("no such route")

      "#{message} (check the API address you configured: it should be the address only, for example " \
        "https://host; a trailing /v1 is accepted)"
    end

    def retry_after(raw)
      value = raw.headers["retry-after"]
      value && Transport.parse_retry_after(value)
    end

    # The code the API would send for a status when the error body is missing (a proxy, an empty 5xx).
    def fallback_code(status)
      case status
      when 401, 403 then "unauthenticated"
      when 402 then "insufficient_credits"
      when 429 then "rate_limited"
      when 500.. then "internal_error"
      else "validation_error"
      end
    end

    # Fields a successful body must carry: "a.b" => "string|int|bool|array|object"; a trailing "?" allows null or absent.
    def shape_problem(body, shape)
      shape.each do |path, spec|
        nullable = spec.end_with?("?")
        type = spec.delete_suffix("?")
        value = path.split(".").reduce(body) { |memo, part| memo.is_a?(Hash) ? memo[part] : nil }
        if value.nil?
          next if nullable

          return "field #{path.inspect} is missing"
        end
        return "field #{path.inspect} is not #{type}" unless type_ok?(type, value)
      end
      nil
    end

    INT64 = (-(2**63)..((2**63) - 1))
    private_constant :INT64

    def type_ok?(type, value)
      case type
      when "string" then value.is_a?(String)
      when "bool" then [true, false].include?(value)
      when "array" then value.is_a?(Array)
      when "object" then value.is_a?(Hash)
      when "int" then value.is_a?(Integer) && INT64.cover?(value)
      else false
      end
    end
  end
end
