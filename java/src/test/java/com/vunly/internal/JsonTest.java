package com.vunly.internal;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;

import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import org.junit.jupiter.api.Test;

class JsonTest {
  @Test
  void parsesEveryValueType() {
    Object v = Json.parse("{\"a\":[1,-2,3.5,1e3,true,false,null,\"x\"],\"b\":{\"c\":\"d\"}}");
    Map<?, ?> m = assertInstanceOf(Map.class, v);
    List<?> a = assertInstanceOf(List.class, m.get("a"));
    assertEquals(1L, a.get(0));
    assertEquals(-2L, a.get(1));
    assertEquals(3.5, a.get(2));
    assertEquals(1000.0, a.get(3));
    assertEquals(true, a.get(4));
    assertEquals(false, a.get(5));
    assertNull(a.get(6));
    assertEquals("x", a.get(7));
    assertEquals("d", ((Map<?, ?>) m.get("b")).get("c"));
  }

  @Test
  void integersBeyondLongBecomeDoubles() {
    assertEquals(9.223372036854775808E18, Json.parse("9223372036854775808"));
    assertEquals(Long.MAX_VALUE, Json.parse("9223372036854775807"));
  }

  @Test
  void decodesEscapesAndSurrogatePairs() {
    assertEquals("a\"b\\c/d\b\f\n\r\t", Json.parse("\"a\\\"b\\\\c\\/d\\b\\f\\n\\r\\t\""));
    assertEquals("\u00e9", Json.parse("\"\\u00e9\""));
    assertEquals("\ud83d\ude00", Json.parse("\"\\ud83d\\ude00\""));
    assertEquals("\u00e9\ud83d\ude00", Json.parse("\"é😀\""));
  }

  @Test
  void rejectsWhatIsNotJson() {
    String[] bad = {
      "", " ", "{", "[1,]", "{\"a\":1,}", "{\"a\" 1}", "{a:1}", "[1 2]", "01", "-", "1.", ".5", "1e", "+1", "tru",
      "nul", "\"abc", "\"\\x\"", "\"\\u12\"", "\"a\nb\"", "1 2", "{} x", "[", "]", "{\"a\":}", "'a'", "NaN",
    };
    for (String s : bad) {
      assertThrows(Json.JsonException.class, () -> Json.parse(s), "should reject: " + s);
    }
  }

  @Test
  void limitsNesting() {
    String deep = "[".repeat(150) + "]".repeat(150);
    assertThrows(Json.JsonException.class, () -> Json.parse(deep));
    String ok = "[".repeat(50) + "]".repeat(50);
    Json.parse(ok);
  }

  @Test
  void duplicateKeysTakeTheLast() {
    assertEquals(2L, ((Map<?, ?>) Json.parse("{\"a\":1,\"a\":2}")).get("a"));
  }

  @Test
  void writesAndRoundTrips() {
    Map<String, Object> m = new LinkedHashMap<>();
    m.put("s", "a\"b\\c\n\u0001é");
    m.put("n", 5L);
    m.put("i", 7);
    m.put("d", 1.5);
    m.put("t", true);
    m.put("z", null);
    m.put("l", List.of(1L, "x", Map.of("k", "v")));
    String text = Json.write(m);
    assertEquals(
        "{\"s\":\"a\\\"b\\\\c\\n\\u0001é\",\"n\":5,\"i\":7,\"d\":1.5,\"t\":true,\"z\":null,\"l\":[1,\"x\",{\"k\":\"v\"}]}", text);
    Map<?, ?> back = (Map<?, ?>) Json.parse(text);
    assertEquals(m.get("s"), back.get("s"));
    assertEquals(5L, back.get("n"));
    assertEquals(7L, back.get("i"));
  }

  @Test
  void refusesToWriteWhatIsNotJson() {
    assertThrows(Json.JsonException.class, () -> Json.write(Double.NaN));
    assertThrows(Json.JsonException.class, () -> Json.write(new Object()));
  }
}
