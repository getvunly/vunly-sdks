<?php

declare(strict_types=1);

namespace Vunly\Laravel;

use Illuminate\Contracts\Config\Repository;
use Illuminate\Routing\Router;
use Illuminate\Support\ServiceProvider;
use Vunly\Client;
use Vunly\Laravel\Http\Middleware\MeterUsage;

/**
 * Registers the Vunly client in the container (one shared instance), the `vunly` route middleware and the config file.
 * Laravel finds this provider by itself (package auto-discovery, `extra.laravel` in composer.json).
 */
final class VunlyServiceProvider extends ServiceProvider
{
    public function register(): void
    {
        $this->mergeConfigFrom(__DIR__ . '/config/vunly.php', 'vunly');

        $this->app->singleton(Client::class, static function ($app): Client {
            /** @var Repository $config */
            $config = $app->make('config');
            $key = $config->get('vunly.api_key');
            $url = $config->get('vunly.base_url');
            if (!is_string($key) || trim($key) === '') {
                throw new \InvalidArgumentException('Vunly: set VUNLY_API_KEY (config vunly.api_key) in your environment.');
            }
            if (!is_string($url) || trim($url) === '') {
                throw new \InvalidArgumentException('Vunly: set VUNLY_API_URL (config vunly.base_url) in your environment.');
            }

            $timeout = $config->get('vunly.timeout', 10.0);
            $retries = $config->get('vunly.max_retries', 2);

            return new Client(
                apiKey: $key,
                baseUrl: $url,
                timeout: is_numeric($timeout) ? (float) $timeout : 10.0,
                maxRetries: is_numeric($retries) ? (int) $retries : 2,
            );
        });
        $this->app->alias(Client::class, 'vunly');
    }

    public function boot(): void
    {
        if ($this->app->runningInConsole()) {
            $this->publishes([__DIR__ . '/config/vunly.php' => $this->app->configPath('vunly.php')], 'vunly-config');
        }
        /** @var Router $router */
        $router = $this->app->make('router');
        $router->aliasMiddleware('vunly', MeterUsage::class);
    }

    /** @return list<string> */
    public function provides(): array
    {
        return [Client::class, 'vunly'];
    }
}
