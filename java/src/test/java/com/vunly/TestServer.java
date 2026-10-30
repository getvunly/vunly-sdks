package com.vunly;

import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpHandler;
import com.sun.net.httpserver.HttpServer;
import java.io.IOException;
import java.io.OutputStream;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.atomic.AtomicInteger;

/** A throw-away local HTTP server for tests: the JDK's own, no dependency. */
final class TestServer implements AutoCloseable {
  static final String KEY = "vk_test_synthetic_000000000000000000";

  final HttpServer server;
  final AtomicInteger hits = new AtomicInteger();

  TestServer(HttpHandler handler) throws IOException {
    server = HttpServer.create(new InetSocketAddress(InetAddress.getLoopbackAddress(), 0), 0);
    server.createContext(
        "/",
        ex -> {
          hits.incrementAndGet();
          try {
            handler.handle(ex);
          } finally {
            ex.close();
          }
        });
    server.start();
  }

  String url() {
    return "http://127.0.0.1:" + server.getAddress().getPort();
  }

  Vunly.Builder client() {
    return Vunly.builder().apiKey(KEY).baseUrl(url()).sleeper(d -> {});
  }

  static void json(HttpExchange ex, int status, String body) throws IOException {
    byte[] b = body.getBytes(StandardCharsets.UTF_8);
    ex.getResponseHeaders().set("Content-Type", "application/json");
    ex.getResponseHeaders().set("Request-Id", "req_test");
    ex.sendResponseHeaders(status, b.length == 0 ? -1 : b.length);
    if (b.length > 0) {
      try (OutputStream os = ex.getResponseBody()) {
        os.write(b);
      }
    }
  }

  static String bodyOf(HttpExchange ex) throws IOException {
    return new String(ex.getRequestBody().readAllBytes(), StandardCharsets.UTF_8);
  }

  @Override
  public void close() {
    server.stop(0);
  }

  static final String ENTRY =
      "{\"id\":\"led_1\",\"customerId\":\"cus_1\",\"kind\":\"debit\",\"credits\":-3,\"createdAt\":\"2026-01-01T00:00:00Z\"}";
}
