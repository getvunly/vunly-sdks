using System;
using System.Collections.Generic;
using System.IO;
using System.Net;
using System.Net.Sockets;
using System.Security.Cryptography;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using Xunit;

namespace Vunly.Tests;

public class VunlyTests
{
    private static Task<UsageResult> Record(VunlyClient v) => v.Usage.RecordAsync("job-12345678", "acme", "tokens", 3000);

    [Fact]
    public void ClientRequiresKeyAndAddressAndNeverPrintsTheKey()
    {
        Assert.Throws<ArgumentException>(() => new VunlyClient(new VunlyClientOptions { BaseUrl = "http://127.0.0.1:1" }));
        Assert.Throws<ArgumentException>(() => new VunlyClient(new VunlyClientOptions { ApiKey = "k" }));
        Assert.Throws<ArgumentException>(() => new VunlyClient(new VunlyClientOptions { ApiKey = "k", BaseUrl = "not a url" }));
        Assert.Throws<ArgumentException>(() => new VunlyClient(new VunlyClientOptions { ApiKey = "k", BaseUrl = "ftp://host" }));
        Assert.Throws<ArgumentException>(() => new VunlyClient(new VunlyClientOptions { ApiKey = "k", BaseUrl = "http://h", MaxRetries = -1 }));
        using var v = new VunlyClient(new VunlyClientOptions { ApiKey = TestServer.Key, BaseUrl = "http://127.0.0.1:1" });
        Assert.DoesNotContain(TestServer.Key, v.ToString());
        Assert.DoesNotContain(TestServer.Key, $"{v}");
    }

    [Fact]
    public void BaseUrlAcceptsTrailingSlashAndV1()
    {
        Assert.Equal("https://api.example.test", VunlyClient.NormalizeBaseUrl("https://api.example.test/v1/"));
        Assert.Equal("https://api.example.test", VunlyClient.NormalizeBaseUrl("https://api.example.test/"));
        Assert.Equal("https://api.example.test", VunlyClient.NormalizeBaseUrl("  https://api.example.test/V1 "));
    }

    [Fact]
    public async Task RecordSendsKeyAndIdempotencyAndReportsOverage()
    {
        using var s = new TestServer(async ctx =>
        {
            Assert.Equal("Bearer " + TestServer.Key, ctx.Request.Headers["Authorization"]);
            Assert.Equal("job-12345678", ctx.Request.Headers["Idempotency-Key"]);
            Assert.StartsWith("vunly-dotnet/", ctx.Request.Headers["User-Agent"]);
            Assert.Contains("\"quantity\":3000", await TestServer.Body(ctx));
            await TestServer.Json(ctx, 200, "{\"event\":" + TestServer.Entry + ",\"balance\":97}", ("Vunly-Plan-Overage", "true"));
        });
        var r = await Record(s.Client());
        Assert.Equal(97, r.Remaining);
        Assert.True(r.PlanOverage);
        Assert.False(r.Replayed);
        Assert.Equal("req_test", r.RequestId);
        Assert.Equal(-3, r.Event.Credits);
    }

    [Fact]
    public async Task ReplayIsReportedAndNeverFlagsOverage()
    {
        using var s = new TestServer(ctx => TestServer.Json(ctx, 200, "{\"event\":" + TestServer.Entry + ",\"balance\":97}", ("Idempotent-Replayed", "true"), ("Vunly-Plan-Overage", "true")));
        var r = await Record(s.Client());
        Assert.True(r.Replayed);
        Assert.False(r.PlanOverage);
    }

    [Fact]
    public async Task BadIdIsRefusedBeforeAnyRequest()
    {
        using var s = new TestServer(ctx => TestServer.Json(ctx, 200, "{}"));
        var e = await Assert.ThrowsAsync<InvalidRequestException>(() => s.Client().Usage.RecordAsync("short", "a", "m", 1));
        Assert.Equal(400, e.Status);
        Assert.Equal("validation_error", e.Code);
        Assert.Equal(0, s.Hits);
    }

    [Fact]
    public async Task LimitReachedCarriesBalanceAndIsNotRetried()
    {
        using var s = new TestServer(ctx => TestServer.Json(ctx, 402, "{\"error\":{\"code\":\"insufficient_credits\",\"message\":\"not enough\",\"details\":{\"balance\":2,\"required\":5}}}"));
        var e = await Assert.ThrowsAsync<LimitReachedException>(() => Record(s.Client()));
        Assert.Equal(2, e.Balance);
        Assert.Equal(5, e.Required);
        Assert.Equal("req_test", e.RequestId);
        Assert.Equal("insufficient_credits", e.Code);
        Assert.Equal(1, s.Hits);
        Assert.DoesNotContain(TestServer.Key, e.Message);
    }

    [Theory]
    [InlineData(401, true)]
    [InlineData(403, true)]
    [InlineData(400, false)]
    [InlineData(404, false)]
    [InlineData(409, false)]
    public async Task AuthAndInvalidRequestClassesAreNotRetried(int status, bool auth)
    {
        using var s = new TestServer(ctx => TestServer.Json(ctx, status, "{\"error\":{\"code\":\"x\",\"message\":\"m\"}}"));
        var e = await Assert.ThrowsAnyAsync<VunlyException>(() => s.Client().Customers.GetAsync("acme"));
        if (auth)
        {
            Assert.IsType<AuthException>(e);
        }
        else
        {
            Assert.IsType<InvalidRequestException>(e);
        }
        Assert.Equal(1, s.Hits);
    }

    [Fact]
    public async Task ReadsRetryServerErrorsThenSucceed()
    {
        var n = 0;
        using var s = new TestServer(ctx => Interlocked.Increment(ref n) < 3
            ? TestServer.Json(ctx, 503, "{\"error\":{\"code\":\"unavailable\",\"message\":\"later\"}}")
            : TestServer.Json(ctx, 200, "{\"credits\":5,\"consumed\":1,\"granted\":6}"));
        var b = await s.Client().Credits.GetBalanceAsync("acme");
        Assert.Equal(5, b.Remaining);
        Assert.Equal(3, s.Hits);
    }

    [Fact]
    public async Task RetriesAreBounded()
    {
        using var s = new TestServer(ctx => TestServer.Json(ctx, 500, "{\"error\":{\"code\":\"internal_error\",\"message\":\"boom\"}}"));
        await Assert.ThrowsAsync<UnavailableException>(() => s.Client(o => o.MaxRetries = 2).Credits.GetBalanceAsync("acme"));
        Assert.Equal(3, s.Hits);
    }

    [Fact]
    public async Task MaxRetriesZeroMeansOneAttempt()
    {
        using var s = new TestServer(ctx => TestServer.Json(ctx, 500, "{\"error\":{\"code\":\"internal_error\",\"message\":\"boom\"}}"));
        await Assert.ThrowsAsync<UnavailableException>(() => s.Client(o => o.MaxRetries = 0).Credits.GetBalanceAsync("acme"));
        Assert.Equal(1, s.Hits);
    }

    [Fact]
    public async Task ClassXIsRetriedOnlyAfter429()
    {
        using (var s = new TestServer(ctx => TestServer.Json(ctx, 500, "{\"error\":{\"code\":\"internal_error\",\"message\":\"boom\"}}")))
        {
            await Assert.ThrowsAsync<UnavailableException>(() => s.Client().CustomerTokens.CreateAsync("acme"));
            Assert.Equal(1, s.Hits);
        }
        var n = 0;
        using (var s = new TestServer(ctx => Interlocked.Increment(ref n) == 1
            ? TestServer.Json(ctx, 429, "{\"error\":{\"code\":\"rate_limited\",\"message\":\"slow\"}}", ("Retry-After", "1"))
            : TestServer.Json(ctx, 200, "{\"id\":\"ct_1\",\"token\":\"vct_secret_value\",\"customerId\":\"cus_1\",\"expiresAt\":\"2026-01-01T00:15:00Z\"}")))
        {
            var t = await s.Client().CustomerTokens.CreateAsync("acme");
            Assert.Equal(2, s.Hits);
            Assert.DoesNotContain("vct_secret_value", t.ToString());
            Assert.Equal("vct_secret_value", t.Token);
        }
    }

    [Fact]
    public async Task PlanVolumeStopIsNeverRetried()
    {
        using var s = new TestServer(ctx => TestServer.Json(ctx, 429, "{\"error\":{\"code\":\"rate_limited\",\"message\":\"plan\",\"details\":{\"reason\":\"plan_included_volume\"}}}", ("Retry-After", "5")));
        var e = await Assert.ThrowsAsync<UnavailableException>(() => Record(s.Client()));
        Assert.True(e.IsPlanStop);
        Assert.Equal(1, s.Hits);
    }

    [Fact]
    public async Task RetryAfterAboveTheCapIsSurfaced()
    {
        using var s = new TestServer(ctx => TestServer.Json(ctx, 429, "{\"error\":{\"code\":\"rate_limited\",\"message\":\"slow\"}}", ("Retry-After", "120")));
        var e = await Assert.ThrowsAsync<UnavailableException>(() => s.Client().Credits.GetBalanceAsync("acme"));
        Assert.Equal(TimeSpan.FromSeconds(120), e.RetryAfter);
        Assert.Equal(1, s.Hits);
    }

    [Fact]
    public void BackoffStaysInItsBounds()
    {
        for (var attempt = 1; attempt <= 10; attempt++)
        {
            var cap = Math.Min(8000d, 250d * Math.Pow(2, attempt - 1));
            for (var i = 0; i < 200; i++)
            {
                var ms = Transport.RetryDelay(attempt, null)!.Value.TotalMilliseconds;
                Assert.InRange(ms, cap / 2, cap);
            }
        }
    }

    [Fact]
    public async Task MalformedSuccessIsInvalidResponse()
    {
        using (var s = new TestServer(ctx => TestServer.Json(ctx, 200, "{\"nope\":1}")))
        {
            var e = await Assert.ThrowsAsync<UnavailableException>(() => s.Client(o => o.MaxRetries = 0).Credits.GetBalanceAsync("acme"));
            Assert.Equal("invalid_response", e.Code);
        }
        using (var s = new TestServer(ctx => TestServer.Json(ctx, 200, "not json")))
        {
            var e = await Assert.ThrowsAsync<UnavailableException>(() => s.Client().Credits.GetBalanceAsync("acme"));
            Assert.Equal("invalid_response", e.Code);
            Assert.Equal(1, s.Hits); // an invalid response is not retried
        }
    }

    [Fact]
    public async Task AMissingRequiredFieldNamesIt()
    {
        using var s = new TestServer(ctx => TestServer.Json(ctx, 200, "{\"event\":{\"id\":\"led_1\"},\"balance\":1}"));
        var e = await Assert.ThrowsAsync<UnavailableException>(() => Record(s.Client()));
        Assert.Equal("invalid_response", e.Code);
        Assert.Contains("event.customerId", e.Message);
    }

    [Fact]
    public async Task UnknownFieldsAreIgnored()
    {
        using var s = new TestServer(ctx => TestServer.Json(ctx, 200, "{\"credits\":5,\"consumed\":1,\"granted\":6,\"newField\":{\"a\":1}}"));
        Assert.Equal(5, (await s.Client().Credits.GetBalanceAsync("acme")).Remaining);
    }

    [Fact]
    public async Task TimeoutIsUnavailable()
    {
        using var s = new TestServer(async ctx => await Task.Delay(2000));
        using var v = s.Client(o =>
        {
            o.Timeout = TimeSpan.FromMilliseconds(100);
            o.MaxRetries = 0;
        });
        var watch = System.Diagnostics.Stopwatch.StartNew();
        var e = await Assert.ThrowsAsync<UnavailableException>(() => v.Credits.GetBalanceAsync("acme"));
        Assert.True(watch.ElapsedMilliseconds < 1500);
        Assert.Equal("unavailable", e.Code);
    }

    [Fact]
    public async Task CancellationThrowsOperationCanceledNotAnOutage()
    {
        using var s = new TestServer(async ctx => await Task.Delay(2000));
        using var v = s.Client();
        using var cts = new CancellationTokenSource(TimeSpan.FromMilliseconds(100));
        await Assert.ThrowsAnyAsync<OperationCanceledException>(() => v.Credits.GetBalanceAsync("acme", cts.Token));
    }

    [Fact]
    public async Task RedirectsAreNotFollowed()
    {
        var followed = 0;
        using var target = new TestServer(ctx =>
        {
            Interlocked.Increment(ref followed);
            return Task.CompletedTask;
        });
        using var s = new TestServer(ctx =>
        {
            ctx.Response.StatusCode = 302;
            ctx.Response.Headers["Location"] = target.Url + "/x";
            return Task.CompletedTask;
        });
        await Assert.ThrowsAnyAsync<VunlyException>(() => s.Client().Credits.GetBalanceAsync("acme"));
        Assert.Equal(0, followed);
    }

    [Fact]
    public async Task PagingIsLazy()
    {
        var pages = 0;
        using var s = new TestServer(ctx =>
        {
            var n = Interlocked.Increment(ref pages);
            var next = n < 3 ? "\"c" + n + "\"" : "null";
            return TestServer.Json(ctx, 200, "{\"data\":[" + TestServer.Entry + "," + TestServer.Entry + "],\"nextCursor\":" + next + "}");
        });
        using var v = s.Client();
        var count = 0;
        await foreach (var e in v.Credits.GetLedgerAllAsync("acme", 2))
        {
            Assert.NotNull(e.Id);
            if (++count == 3)
            {
                break;
            }
        }
        Assert.Equal(3, count);
        Assert.Equal(2, pages); // stopping early stops fetching

        pages = 0;
        var all = 0;
        await foreach (var _ in v.Credits.GetLedgerAllAsync("acme", 2))
        {
            all++;
        }
        Assert.Equal(6, all);
        Assert.Equal(3, pages);
    }

    [Fact]
    public async Task PathSegmentsAreEscaped()
    {
        string? raw = null;
        using var s = new TestServer(ctx =>
        {
            raw = ctx.Request.RawUrl;
            return TestServer.Json(ctx, 200, "{\"credits\":0,\"consumed\":0,\"granted\":0}");
        });
        await s.Client().Credits.GetBalanceAsync("a/b?c");
        Assert.Equal("/v1/customers/a%2Fb%3Fc/balance", raw);
    }

    /// <summary>
    /// A keyed request whose connection dropped must not be sent twice behind the SDK's back (Go's net/http did; the JDK's
    /// client does not). A raw TCP server answers a GET with keep-alive and then drops every POST after reading it.
    /// </summary>
    [Fact]
    public async Task ADroppedKeyedRequestIsNotReplayedByTheTransport()
    {
        var posts = 0;
        var listener = new TcpListener(IPAddress.Loopback, 0);
        listener.Start();
        var port = ((IPEndPoint)listener.LocalEndpoint).Port;
        var accept = Task.Run(async () =>
        {
            try
            {
                while (true)
                {
                    var c = await listener.AcceptTcpClientAsync();
                    _ = Task.Run(() => Serve(c, () => Interlocked.Increment(ref posts)));
                }
            }
            catch (Exception)
            {
                // listener stopped by the test
            }
        });
        try
        {
            using var v = new VunlyClient(new VunlyClientOptions { ApiKey = TestServer.Key, BaseUrl = $"http://127.0.0.1:{port}", MaxRetries = 0 });
            Assert.Equal(1, (await v.Credits.GetBalanceAsync("acme")).Remaining); // warms a keep-alive connection
            await Assert.ThrowsAsync<UnavailableException>(() => Record(v));
            Assert.Equal(1, Volatile.Read(ref posts));
        }
        finally
        {
            listener.Stop();
            await accept;
        }
    }

    private static async Task Serve(TcpClient c, Action onPost)
    {
        using (c)
        {
            var stream = c.GetStream();
            using var reader = new StreamReader(stream, Encoding.ASCII, false, 1024, leaveOpen: true);
            while (true)
            {
                var line = await reader.ReadLineAsync();
                if (line is null)
                {
                    return;
                }
                var post = line.StartsWith("POST", StringComparison.Ordinal);
                var length = 0;
                for (var h = await reader.ReadLineAsync(); !string.IsNullOrEmpty(h); h = await reader.ReadLineAsync())
                {
                    if (h.StartsWith("content-length:", StringComparison.OrdinalIgnoreCase))
                    {
                        length = int.Parse(h[15..].Trim(), System.Globalization.CultureInfo.InvariantCulture);
                    }
                }
                var buf = new char[length];
                var read = 0;
                while (read < length)
                {
                    var n = await reader.ReadAsync(buf.AsMemory(read, length - read));
                    if (n == 0)
                    {
                        break;
                    }
                    read += n;
                }
                if (post)
                {
                    onPost();
                    return; // the request arrived, the answer never will
                }
                var body = Encoding.UTF8.GetBytes("{\"credits\":1,\"consumed\":0,\"granted\":1}");
                var head = Encoding.ASCII.GetBytes($"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {body.Length}\r\nConnection: keep-alive\r\n\r\n");
                await stream.WriteAsync(head);
                await stream.WriteAsync(body);
                await stream.FlushAsync();
            }
        }
    }

    private static string Sign(string secret, long t, string payload) =>
        $"t={t},v1={Convert.ToHexString(HMACSHA256.HashData(Encoding.UTF8.GetBytes(secret), Encoding.UTF8.GetBytes($"{t}.{payload}"))).ToLowerInvariant()}";

    private static string Reason(string payload, string? header, string[] secrets, DateTimeOffset now) =>
        Assert.Throws<WebhookException>(() => Webhooks.Verify(Encoding.UTF8.GetBytes(payload), header, secrets, now)).Reason;

    [Fact]
    public void WebhooksVerifyAndRejectLikeTheOtherSdks()
    {
        const long now = 1_800_000_000L;
        var at = DateTimeOffset.FromUnixTimeSeconds(now);
        const string payload = "{\"id\":\"evt_1\",\"type\":\"credits.low\",\"createdAt\":\"2026-01-01T00:00:00Z\",\"data\":{\"a\":1}}";

        var ev = Webhooks.Verify(Encoding.UTF8.GetBytes(payload), Sign("old", now, payload), new[] { "new", "old" }, at);
        Assert.Equal("evt_1", ev.Id);
        Assert.Equal("credits.low", ev.Type);
        Assert.Equal(1, ev.Data.GetProperty("a").GetInt32());

        Assert.Equal("missing_header", Reason(payload, "", new[] { "s" }, at));
        Assert.Equal("malformed_header", Reason(payload, "garbage", new[] { "s" }, at));
        Assert.Equal("stale_timestamp", Reason(payload, Sign("s", now - 301, payload), new[] { "s" }, at));
        Assert.Equal("stale_timestamp", Reason(payload, Sign("s", now + 301, payload), new[] { "s" }, at));
        Assert.Equal("no_matching_signature", Reason(payload, Sign("other", now, payload), new[] { "s" }, at));
        Assert.Equal("no_matching_signature", Reason(payload + " ", Sign("s", now, payload), new[] { "s" }, at));
        const string bad = "{\"nope\":true}";
        Assert.Equal("invalid_payload", Reason(bad, Sign("s", now, bad), new[] { "s" }, at));
    }

    [Fact]
    public async Task PlanUsageAndSubscriptionParse()
    {
        using var s = new TestServer(ctx => ctx.Request.RawUrl == "/v1/plan-usage"
            ? TestServer.Json(ctx, 200, "{\"plan\":{\"key\":\"free\",\"name\":\"Free\"},\"period\":{\"start\":\"2026-10-01T00:00:00Z\",\"end\":\"2026-11-01T00:00:00Z\"},\"usage\":{\"events\":12},\"unconfirmedLimits\":{\"confirmed\":false,\"note\":\"n\",\"includedMonthlyEvents\":null,\"activityRetentionDays\":7}}")
            : TestServer.Json(ctx, 200, "{\"id\":\"sub_1\",\"customerId\":\"cus_1\",\"status\":\"active\",\"plan\":{\"key\":\"starter\"}}"));
        using var v = s.Client();
        var u = await v.GetPlanUsageAsync();
        Assert.Equal(12, u.Events);
        Assert.False(u.UnconfirmedLimits.Confirmed);
        Assert.Null(u.UnconfirmedLimits.IncludedMonthlyEvents);
        Assert.Equal(7, u.UnconfirmedLimits.ActivityRetentionDays);
        var sub = await v.Plans.SubscribeAsync("acme", "starter");
        Assert.Equal("active", sub.Status);
        Assert.Equal("starter", sub.Plan.Key);
    }
}
