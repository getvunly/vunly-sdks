# frozen_string_literal: true

require "net/http"

module Vunly
  # A {Net::HTTP} whose socket reader refuses what a hostile server can use to exhaust memory BEFORE the body: an endless
  # status line, an endless header line or chunk-size line (Net::HTTP buffers a line until it sees the newline), and
  # endless distinct headers (it keeps them all in a hash). The body itself is bounded separately, by the transport.
  #
  # The limits count bytes the SOCKET delivered, so they hold whatever the server sends:
  # - one line (status, header, chunk size with extensions): at most {MAX_LINE} bytes;
  # - the status line and all headers: at most {MAX_HEAD} bytes and {MAX_HEAD_LINES} lines;
  # - every line read after the body began (chunk sizes, trailers): at most {MAX_BODY_LINES} bytes in total.
  #
  # @api private
  class BoundedHTTP < Net::HTTP
    # Raised when a server exceeds a limit. Internal: the transport turns it into an {UnavailableError}.
    class Overflow < StandardError; end

    # The socket reader.
    class Reader < Net::BufferedIO
      # The longest single line (bytes).
      MAX_LINE = 32 * 1024
      # The status line plus all header lines (bytes).
      MAX_HEAD = 64 * 1024
      # The most lines in the head.
      MAX_HEAD_LINES = 200
      # All chunk-size lines and trailers together (bytes).
      MAX_BODY_LINES = 1024 * 1024

      def initialize(*, **)
        super
        @head_bytes = 0
        @head_lines = 0
        @body_line_bytes = 0
        @body_started = false
      end

      # Marks the start of the body: from now on lines are chunk sizes and trailers.
      def read(*)
        @body_started = true
        super
      end

      # Reads one line, counting it against the budget of its phase.
      def readuntil(*)
        line = super
        count_line(line.bytesize)
        line
      end

      private

      # The terminator was not in what was buffered: a partial line longer than MAX_LINE is a hostile server.
      def rbuf_fill
        super
        raise Overflow, "a line is longer than #{MAX_LINE} bytes" if rbuf_size > MAX_LINE
      end

      def count_line(bytes)
        if @body_started
          @body_line_bytes += bytes
          raise Overflow, "chunk and trailer lines exceed #{MAX_BODY_LINES} bytes" if @body_line_bytes > MAX_BODY_LINES
        else
          @head_bytes += bytes
          @head_lines += 1
          raise Overflow, "the response head exceeds its limits" if @head_bytes > MAX_HEAD || @head_lines > MAX_HEAD_LINES
        end
      end
    end

    private

    def connect
      super
      @socket = Reader.new(@socket.io, read_timeout: @read_timeout, write_timeout: @write_timeout,
                                       continue_timeout: @continue_timeout, debug_output: @debug_output)
    end
  end
end
