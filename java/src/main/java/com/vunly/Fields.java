package com.vunly;

import java.time.Instant;
import java.time.OffsetDateTime;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;

/**
 * Typed access to a parsed JSON object. A required field that is missing or of the wrong type throws
 * {@link IllegalArgumentException}; the transport turns that into {@code invalid_response} (fail closed: never a zero
 * value that looks like an answer). Unknown fields are ignored on purpose (additive API changes must not break clients).
 */
final class Fields {
  private final Map<String, Object> map;
  private final String path;

  private Fields(Map<String, Object> map, String path) {
    this.map = map;
    this.path = path;
  }

  @SuppressWarnings("unchecked")
  static Fields of(Object parsed) {
    if (!(parsed instanceof Map)) {
      throw new IllegalArgumentException("the body is not a JSON object");
    }
    return new Fields((Map<String, Object>) parsed, "");
  }

  private String name(String key) {
    return path + key;
  }

  private IllegalArgumentException bad(String key, String want) {
    return new IllegalArgumentException("field \"" + name(key) + "\" is " + (map.get(key) == null ? "missing" : "not " + want));
  }

  boolean has(String key) {
    return map.get(key) != null;
  }

  String str(String key) {
    if (map.get(key) instanceof String s) {
      return s;
    }
    throw bad(key, "a string");
  }

  /** Null when absent or null. */
  String strOrNull(String key) {
    Object v = map.get(key);
    if (v == null) {
      return null;
    }
    if (v instanceof String s) {
      return s;
    }
    throw bad(key, "a string");
  }

  long lng(String key) {
    if (map.get(key) instanceof Long l) {
      return l;
    }
    throw bad(key, "an integer");
  }

  long lngOr(String key, long fallback) {
    return map.get(key) == null ? fallback : lng(key);
  }

  /** Null when absent or null. */
  Long lngOrNull(String key) {
    return map.get(key) == null ? null : Long.valueOf(lng(key));
  }

  boolean bool(String key) {
    if (map.get(key) instanceof Boolean b) {
      return b;
    }
    throw bad(key, "a boolean");
  }

  Instant instant(String key) {
    try {
      return OffsetDateTime.parse(str(key)).toInstant();
    } catch (java.time.format.DateTimeParseException e) {
      throw bad(key, "an ISO-8601 timestamp");
    }
  }

  /** Null when absent. */
  Instant instantOrNull(String key) {
    return map.get(key) == null ? null : instant(key);
  }

  @SuppressWarnings("unchecked")
  Fields obj(String key) {
    if (map.get(key) instanceof Map) {
      return new Fields((Map<String, Object>) map.get(key), name(key) + ".");
    }
    throw bad(key, "an object");
  }

  @SuppressWarnings("unchecked")
  List<Fields> objects(String key) {
    if (!(map.get(key) instanceof List<?> raw)) {
      throw bad(key, "an array");
    }
    List<Fields> out = new ArrayList<>(raw.size());
    int i = 0;
    for (Object o : raw) {
      if (!(o instanceof Map)) {
        throw new IllegalArgumentException("field \"" + name(key) + "[" + i + "]\" is not an object");
      }
      out.add(new Fields((Map<String, Object>) o, name(key) + "[" + i + "]."));
      i++;
    }
    return out;
  }

  List<String> strings(String key) {
    if (!(map.get(key) instanceof List<?> raw)) {
      throw bad(key, "an array");
    }
    List<String> out = new ArrayList<>(raw.size());
    for (Object o : raw) {
      if (!(o instanceof String s)) {
        throw bad(key, "an array of strings");
      }
      out.add(s);
    }
    return List.copyOf(out);
  }
}
