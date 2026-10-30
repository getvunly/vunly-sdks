using System;
using System.Collections;
using System.Collections.Generic;
using System.IO;
using System.Text.Json;

namespace Vunly;

/// <summary>Writes a request body from plain values with <see cref="Utf8JsonWriter"/>: no reflection, so it is trimming-safe.</summary>
internal static class JsonBody
{
    public static byte[] Write(IReadOnlyDictionary<string, object?> body)
    {
        using var stream = new MemoryStream();
        using (var w = new Utf8JsonWriter(stream))
        {
            Value(w, body);
        }
        return stream.ToArray();
    }

    private static void Value(Utf8JsonWriter w, object? v)
    {
        switch (v)
        {
            case null:
                w.WriteNullValue();
                break;
            case string s:
                w.WriteStringValue(s);
                break;
            case bool b:
                w.WriteBooleanValue(b);
                break;
            case int i:
                w.WriteNumberValue(i);
                break;
            case long l:
                w.WriteNumberValue(l);
                break;
            case IReadOnlyDictionary<string, object?> d:
                w.WriteStartObject();
                foreach (var kv in d)
                {
                    w.WritePropertyName(kv.Key);
                    Value(w, kv.Value);
                }
                w.WriteEndObject();
                break;
            case IEnumerable e:
                w.WriteStartArray();
                foreach (var item in e)
                {
                    Value(w, item);
                }
                w.WriteEndArray();
                break;
            default:
                throw new ArgumentException($"cannot write a {v.GetType().Name} as JSON");
        }
    }
}
