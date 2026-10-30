using System;
using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Threading;
using System.Threading.Tasks;

namespace Vunly.Tests;

/// <summary>A throw-away local HTTP server for tests: the framework's own HttpListener, no dependency.</summary>
internal sealed class TestServer : IDisposable
{
    internal const string Key = "vk_test_synthetic_000000000000000000";
    internal const string Entry = "{\"id\":\"led_1\",\"customerId\":\"cus_1\",\"kind\":\"debit\",\"credits\":-3,\"createdAt\":\"2026-01-01T00:00:00Z\"}";

    private readonly HttpListener _listener = new();
    private readonly CancellationTokenSource _stop = new();
    private int _hits;

    internal TestServer(Func<HttpListenerContext, Task> handler)
    {
        var probe = new TcpListener(IPAddress.Loopback, 0);
        probe.Start();
        var port = ((IPEndPoint)probe.LocalEndpoint).Port;
        probe.Stop();
        Url = $"http://127.0.0.1:{port}";
        _listener.Prefixes.Add(Url + "/");
        _listener.Start();
        _ = Task.Run(async () =>
        {
            while (!_stop.IsCancellationRequested)
            {
                HttpListenerContext ctx;
                try
                {
                    ctx = await _listener.GetContextAsync().ConfigureAwait(false);
                }
                catch (Exception) when (_stop.IsCancellationRequested || !_listener.IsListening)
                {
                    return;
                }
                Interlocked.Increment(ref _hits);
                _ = Task.Run(async () =>
                {
                    try
                    {
                        await handler(ctx).ConfigureAwait(false);
                    }
                    catch (Exception)
                    {
                        // a test handler that was cut off by Dispose
                    }
                    finally
                    {
                        try
                        {
                            ctx.Response.Close();
                        }
                        catch (Exception)
                        {
                            // already closed
                        }
                    }
                });
            }
        });
    }

    internal string Url { get; }

    internal int Hits => Volatile.Read(ref _hits);

    internal VunlyClient Client(Action<VunlyClientOptions>? configure = null)
    {
        var o = new VunlyClientOptions { ApiKey = Key, BaseUrl = Url };
        configure?.Invoke(o);
        return new VunlyClient(o, null, (_, _) => Task.CompletedTask);
    }

    internal static async Task Json(HttpListenerContext ctx, int status, string body, params (string, string)[] headers)
    {
        var bytes = Encoding.UTF8.GetBytes(body);
        ctx.Response.StatusCode = status;
        ctx.Response.ContentType = "application/json";
        ctx.Response.Headers["Request-Id"] = "req_test";
        foreach (var (k, v) in headers)
        {
            ctx.Response.Headers[k] = v;
        }
        ctx.Response.ContentLength64 = bytes.Length;
        await ctx.Response.OutputStream.WriteAsync(bytes).ConfigureAwait(false);
    }

    internal static async Task<string> Body(HttpListenerContext ctx)
    {
        using var reader = new System.IO.StreamReader(ctx.Request.InputStream, Encoding.UTF8);
        return await reader.ReadToEndAsync().ConfigureAwait(false);
    }

    public void Dispose()
    {
        _stop.Cancel();
        _listener.Close();
        _stop.Dispose();
    }
}
