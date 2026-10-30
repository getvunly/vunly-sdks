<?php

declare(strict_types=1);

namespace Vunly\Tests\Support;

/** Starts tests/Support/fake-server.php with a script and reads back what it received. */
final class FakeServer
{
    /** @var resource|null */
    private $process;
    private string $dir;
    public readonly string $url;

    /** @param list<array<string, mixed>> $script */
    public function __construct(array $script)
    {
        $this->dir = sys_get_temp_dir() . '/vunly-fake-' . bin2hex(random_bytes(6));
        mkdir($this->dir, 0700, true);
        file_put_contents("{$this->dir}/script.json", json_encode($script, JSON_THROW_ON_ERROR));
        touch("{$this->dir}/log.jsonl");
        $cmd = [PHP_BINARY, __DIR__ . '/fake-server.php', "{$this->dir}/script.json", "{$this->dir}/log.jsonl", "{$this->dir}/port"];
        $process = proc_open($cmd, [0 => ['file', '/dev/null', 'r'], 1 => ['file', '/dev/null', 'w'], 2 => ['file', "{$this->dir}/stderr", 'w']], $pipes);
        if ($process === false) {
            throw new \RuntimeException('cannot start the fake server');
        }
        $this->process = $process;
        for ($i = 0; $i < 200; $i++) {
            $port = @file_get_contents("{$this->dir}/port");
            if (is_string($port) && $port !== '') {
                $this->url = "http://127.0.0.1:$port";

                return;
            }
            usleep(25_000);
        }
        $this->stop();
        throw new \RuntimeException('the fake server did not start');
    }

    /** @return list<array{method: string, path: string, headers: array<string, string>, body: string}> */
    public function requests(): array
    {
        $out = [];
        foreach (file("{$this->dir}/log.jsonl", FILE_IGNORE_NEW_LINES | FILE_SKIP_EMPTY_LINES) ?: [] as $line) {
            /** @var array{method: string, path: string, headers: array<string, string>, body: string} $r */
            $r = json_decode($line, true, 64, JSON_THROW_ON_ERROR);
            $out[] = $r;
        }

        return $out;
    }

    public function stop(): void
    {
        if (is_resource($this->process)) {
            proc_terminate($this->process, 9);
            proc_close($this->process);
        }
        $this->process = null;
        foreach (glob("{$this->dir}/*") ?: [] as $f) {
            @unlink($f);
        }
        @rmdir($this->dir);
    }

    public function __destruct()
    {
        $this->stop();
    }
}
