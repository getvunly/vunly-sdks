package com.vunly.internal;

import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * A small, strict JSON codec (RFC 8259). The JDK has none, and a dependency such as Jackson would collide with the
 * version an application (Spring, for one) already uses. Not part of the public API: it may change without notice.
 *
 * <p>Parsed values are {@code Map<String,Object>} (insertion ordered), {@code List<Object>}, {@code String},
 * {@code Long} (an integer that fits), {@code Double}, {@code Boolean} and {@code null}.
 */
public final class Json {
  private static final int MAX_DEPTH = 100;

  private Json() {}

  /** Thrown for text that is not valid JSON. */
  public static final class JsonException extends RuntimeException {
    private static final long serialVersionUID = 1L;

    JsonException(String message) {
      super(message);
    }
  }

  public static Object parse(byte[] utf8) {
    return parse(new String(utf8, StandardCharsets.UTF_8));
  }

  public static Object parse(String text) {
    Parser p = new Parser(text);
    p.skipWs();
    Object v = p.value(0);
    p.skipWs();
    if (p.pos != text.length()) {
      throw p.error("unexpected data after the value");
    }
    return v;
  }

  public static String write(Object value) {
    StringBuilder sb = new StringBuilder();
    write(sb, value);
    return sb.toString();
  }

  private static void write(StringBuilder sb, Object v) {
    if (v == null) {
      sb.append("null");
    } else if (v instanceof String s) {
      string(sb, s);
    } else if (v instanceof Boolean b) {
      sb.append(b ? "true" : "false");
    } else if (v instanceof Long || v instanceof Integer || v instanceof Short || v instanceof Byte) {
      sb.append(((Number) v).longValue());
    } else if (v instanceof Double || v instanceof Float) {
      double d = ((Number) v).doubleValue();
      if (Double.isNaN(d) || Double.isInfinite(d)) {
        throw new JsonException("NaN and Infinity cannot be written as JSON");
      }
      sb.append(d);
    } else if (v instanceof Map<?, ?> m) {
      sb.append('{');
      boolean first = true;
      for (Map.Entry<?, ?> e : m.entrySet()) {
        if (!first) {
          sb.append(',');
        }
        first = false;
        string(sb, String.valueOf(e.getKey()));
        sb.append(':');
        write(sb, e.getValue());
      }
      sb.append('}');
    } else if (v instanceof Iterable<?> it) {
      sb.append('[');
      boolean first = true;
      for (Object o : it) {
        if (!first) {
          sb.append(',');
        }
        first = false;
        write(sb, o);
      }
      sb.append(']');
    } else {
      throw new JsonException("cannot write a " + v.getClass().getName() + " as JSON");
    }
  }

  private static void string(StringBuilder sb, String s) {
    sb.append('"');
    for (int i = 0; i < s.length(); i++) {
      char c = s.charAt(i);
      switch (c) {
        case '"' -> sb.append("\\\"");
        case '\\' -> sb.append("\\\\");
        case '\b' -> sb.append("\\b");
        case '\f' -> sb.append("\\f");
        case '\n' -> sb.append("\\n");
        case '\r' -> sb.append("\\r");
        case '\t' -> sb.append("\\t");
        default -> {
          if (c < 0x20) {
            sb.append(String.format("\\u%04x", (int) c));
          } else {
            sb.append(c);
          }
        }
      }
    }
    sb.append('"');
  }

  private static final class Parser {
    final String s;
    int pos;

    Parser(String s) {
      this.s = s;
    }

    JsonException error(String what) {
      return new JsonException("invalid JSON at position " + pos + ": " + what);
    }

    void skipWs() {
      while (pos < s.length()) {
        char c = s.charAt(pos);
        if (c == ' ' || c == '\t' || c == '\n' || c == '\r') {
          pos++;
        } else {
          break;
        }
      }
    }

    Object value(int depth) {
      if (depth > MAX_DEPTH) {
        throw error("nesting is too deep");
      }
      if (pos >= s.length()) {
        throw error("unexpected end");
      }
      char c = s.charAt(pos);
      switch (c) {
        case '{':
          return object(depth);
        case '[':
          return array(depth);
        case '"':
          return string();
        case 't':
          return literal("true", Boolean.TRUE);
        case 'f':
          return literal("false", Boolean.FALSE);
        case 'n':
          return literal("null", null);
        default:
          if (c == '-' || (c >= '0' && c <= '9')) {
            return number();
          }
          throw error("unexpected character '" + c + "'");
      }
    }

    Object literal(String word, Object result) {
      if (!s.startsWith(word, pos)) {
        throw error("expected " + word);
      }
      pos += word.length();
      return result;
    }

    Map<String, Object> object(int depth) {
      Map<String, Object> m = new LinkedHashMap<>();
      pos++; // {
      skipWs();
      if (pos < s.length() && s.charAt(pos) == '}') {
        pos++;
        return m;
      }
      while (true) {
        skipWs();
        if (pos >= s.length() || s.charAt(pos) != '"') {
          throw error("expected a string key");
        }
        String key = string();
        skipWs();
        if (pos >= s.length() || s.charAt(pos) != ':') {
          throw error("expected ':'");
        }
        pos++;
        skipWs();
        m.put(key, value(depth + 1));
        skipWs();
        if (pos >= s.length()) {
          throw error("unexpected end in an object");
        }
        char c = s.charAt(pos++);
        if (c == '}') {
          return m;
        }
        if (c != ',') {
          pos--;
          throw error("expected ',' or '}'");
        }
      }
    }

    List<Object> array(int depth) {
      List<Object> l = new ArrayList<>();
      pos++; // [
      skipWs();
      if (pos < s.length() && s.charAt(pos) == ']') {
        pos++;
        return l;
      }
      while (true) {
        skipWs();
        l.add(value(depth + 1));
        skipWs();
        if (pos >= s.length()) {
          throw error("unexpected end in an array");
        }
        char c = s.charAt(pos++);
        if (c == ']') {
          return l;
        }
        if (c != ',') {
          pos--;
          throw error("expected ',' or ']'");
        }
      }
    }

    String string() {
      pos++; // opening quote
      StringBuilder sb = new StringBuilder();
      while (true) {
        if (pos >= s.length()) {
          throw error("unterminated string");
        }
        char c = s.charAt(pos++);
        if (c == '"') {
          return sb.toString();
        }
        if (c < 0x20) {
          pos--;
          throw error("raw control character in a string");
        }
        if (c != '\\') {
          sb.append(c);
          continue;
        }
        if (pos >= s.length()) {
          throw error("unterminated escape");
        }
        char e = s.charAt(pos++);
        switch (e) {
          case '"' -> sb.append('"');
          case '\\' -> sb.append('\\');
          case '/' -> sb.append('/');
          case 'b' -> sb.append('\b');
          case 'f' -> sb.append('\f');
          case 'n' -> sb.append('\n');
          case 'r' -> sb.append('\r');
          case 't' -> sb.append('\t');
          case 'u' -> {
            if (pos + 4 > s.length()) {
              throw error("short \\u escape");
            }
            int cp = 0;
            for (int i = 0; i < 4; i++) {
              int d = Character.digit(s.charAt(pos + i), 16);
              if (d < 0) {
                throw error("bad \\u escape");
              }
              cp = cp * 16 + d;
            }
            pos += 4;
            sb.append((char) cp);
          }
          default -> {
            pos--;
            throw error("bad escape '\\" + e + "'");
          }
        }
      }
    }

    Object number() {
      int start = pos;
      if (s.charAt(pos) == '-') {
        pos++;
      }
      if (pos >= s.length()) {
        throw error("bad number");
      }
      if (s.charAt(pos) == '0') {
        pos++;
      } else if (s.charAt(pos) >= '1' && s.charAt(pos) <= '9') {
        while (pos < s.length() && Character.isDigit(s.charAt(pos)) && s.charAt(pos) < 128) {
          pos++;
        }
      } else {
        throw error("bad number");
      }
      boolean integral = true;
      if (pos < s.length() && s.charAt(pos) == '.') {
        integral = false;
        pos++;
        int digits = pos;
        while (pos < s.length() && s.charAt(pos) >= '0' && s.charAt(pos) <= '9') {
          pos++;
        }
        if (pos == digits) {
          throw error("bad number");
        }
      }
      if (pos < s.length() && (s.charAt(pos) == 'e' || s.charAt(pos) == 'E')) {
        integral = false;
        pos++;
        if (pos < s.length() && (s.charAt(pos) == '+' || s.charAt(pos) == '-')) {
          pos++;
        }
        int digits = pos;
        while (pos < s.length() && s.charAt(pos) >= '0' && s.charAt(pos) <= '9') {
          pos++;
        }
        if (pos == digits) {
          throw error("bad number");
        }
      }
      String text = s.substring(start, pos);
      if (integral) {
        try {
          return Long.parseLong(text);
        } catch (NumberFormatException tooBig) {
          return Double.parseDouble(text);
        }
      }
      return Double.parseDouble(text);
    }
  }
}
