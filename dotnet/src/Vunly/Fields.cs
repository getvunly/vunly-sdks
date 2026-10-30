using System;
using System.Collections.Generic;
using System.Globalization;
using System.Text.Json;

namespace Vunly;

/// <summary>
/// Typed access to a parsed JSON object. A required field that is missing or of the wrong type throws
/// <see cref="FormatException"/>; the transport turns that into <c>invalid_response</c> (fail closed: never a zero value that
/// looks like an answer). Unknown fields are ignored on purpose: additive API changes must not break clients.
/// </summary>
internal readonly struct Fields
{
    private readonly JsonElement _e;
    private readonly string _base;
    private readonly int _index;

    // The path of an element of a list is built only when a message needs it (a list of 100,000 elements must not allocate
    // 100,000 path strings): a shared base ("data") plus the index.
    private Fields(JsonElement e, string basePath, int index = -1)
    {
        _e = e;
        _base = basePath;
        _index = index;
    }

    private string _path => _index < 0 ? _base : $"{_base}[{_index}].";

    public static Fields Of(JsonElement root)
    {
        if (root.ValueKind != JsonValueKind.Object)
        {
            throw new FormatException("the body is not a JSON object");
        }
        return new Fields(root, "");
    }

    private bool TryGet(string key, out JsonElement value) =>
        _e.TryGetProperty(key, out value) && value.ValueKind != JsonValueKind.Null && value.ValueKind != JsonValueKind.Undefined;

    private FormatException Bad(string key, string want) =>
        new($"field \"{_path}{key}\" is {(TryGet(key, out _) ? "not " + want : "missing")}");

    public bool Has(string key) => TryGet(key, out _);

    public string Str(string key) =>
        TryGet(key, out var v) && v.ValueKind == JsonValueKind.String ? v.GetString()! : throw Bad(key, "a string");

    public string? StrOrNull(string key) => Has(key) ? Str(key) : null;

    public long Long(string key) =>
        TryGet(key, out var v) && v.ValueKind == JsonValueKind.Number && v.TryGetInt64(out var l) ? l : throw Bad(key, "an integer");

    public long LongOr(string key, long fallback) => Has(key) ? Long(key) : fallback;

    public long? LongOrNull(string key) => Has(key) ? Long(key) : null;

    public bool Bool(string key) =>
        TryGet(key, out var v) && (v.ValueKind == JsonValueKind.True || v.ValueKind == JsonValueKind.False)
            ? v.GetBoolean()
            : throw Bad(key, "a boolean");

    public DateTimeOffset Time(string key) =>
        DateTimeOffset.TryParse(Str(key), CultureInfo.InvariantCulture, DateTimeStyles.AssumeUniversal, out var t)
            ? t.ToUniversalTime()
            : throw Bad(key, "an ISO-8601 timestamp");

    public DateTimeOffset? TimeOrNull(string key) => Has(key) ? Time(key) : null;

    public Fields Obj(string key) =>
        TryGet(key, out var v) && v.ValueKind == JsonValueKind.Object ? new Fields(v, _path + key + ".") : throw Bad(key, "an object");

    public List<Fields> Objects(string key)
    {
        if (!TryGet(key, out var v) || v.ValueKind != JsonValueKind.Array)
        {
            throw Bad(key, "an array");
        }
        var list = new List<Fields>();
        var listBase = _path + key;
        var i = 0;
        foreach (var item in v.EnumerateArray())
        {
            if (item.ValueKind != JsonValueKind.Object)
            {
                throw new FormatException($"field \"{listBase}[{i}]\" is not an object");
            }
            list.Add(new Fields(item, listBase, i));
            i++;
        }
        return list;
    }

    public IReadOnlyList<string> Strings(string key)
    {
        if (!TryGet(key, out var v) || v.ValueKind != JsonValueKind.Array)
        {
            throw Bad(key, "an array");
        }
        var list = new List<string>();
        foreach (var item in v.EnumerateArray())
        {
            if (item.ValueKind != JsonValueKind.String)
            {
                throw Bad(key, "an array of strings");
            }
            list.Add(item.GetString()!);
        }
        return list;
    }

    /// <summary>A JSON value as plain CLR objects: string, long, double, bool, null, List, Dictionary.</summary>
    public static object? ToObject(JsonElement e)
    {
        switch (e.ValueKind)
        {
            case JsonValueKind.String:
                return e.GetString();
            case JsonValueKind.Number:
                return e.TryGetInt64(out var l) ? l : e.GetDouble();
            case JsonValueKind.True:
                return true;
            case JsonValueKind.False:
                return false;
            case JsonValueKind.Array:
                var list = new List<object?>();
                foreach (var item in e.EnumerateArray())
                {
                    list.Add(ToObject(item));
                }
                return list;
            case JsonValueKind.Object:
                var dict = new Dictionary<string, object?>();
                foreach (var p in e.EnumerateObject())
                {
                    dict[p.Name] = ToObject(p.Value);
                }
                return dict;
            default:
                return null;
        }
    }
}
