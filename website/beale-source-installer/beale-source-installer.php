<?php
/**
 * Plugin Name: Beale Source Installer
 * Description: Serves the Beale source installer and latest desktop downloads.
 * Version: 1.1.0
 * Requires PHP: 7.4
 */

if (!defined('ABSPATH')) {
    exit;
}

function beale_latest_desktop_assets()
{
    $cached = get_transient('beale_latest_desktop_assets');
    if (is_array($cached)) {
        return $cached;
    }

    $response = wp_safe_remote_get('https://api.github.com/repos/philo-groves/beale-mono/releases/latest', [
        'timeout' => 5,
        'headers' => [
            'Accept' => 'application/vnd.github+json',
            'User-Agent' => 'Beale-Website',
        ],
    ]);
    if (is_wp_error($response) || wp_remote_retrieve_response_code($response) !== 200) {
        return [];
    }

    $release = json_decode(wp_remote_retrieve_body($response), true);
    $tag = is_array($release) ? ($release['tag_name'] ?? '') : '';
    if (!is_string($tag) || !preg_match('/^v([0-9]+\.[0-9]+\.[0-9]+)$/D', $tag, $matches)
        || !empty($release['draft']) || !empty($release['prerelease']) || !isset($release['assets'])
        || !is_array($release['assets'])) {
        return [];
    }

    $version = $matches[1];
    $expected = [
        'macos' => "Beale-{$version}-arm64.dmg",
        'windows' => "Beale-Setup-{$version}.exe",
    ];
    $assets = [];
    foreach ($release['assets'] as $asset) {
        if (!is_array($asset) || !isset($asset['name'], $asset['browser_download_url'])) {
            continue;
        }
        foreach ($expected as $platform => $filename) {
            $url = 'https://github.com/philo-groves/beale-mono/releases/download/' . $tag . '/' . $filename;
            if ($asset['name'] === $filename && $asset['browser_download_url'] === $url) {
                $assets[$platform] = $url;
            }
        }
    }

    // Keep GitHub API use below its unauthenticated rate limit, while picking up new releases promptly.
    set_transient('beale_latest_desktop_assets', $assets, 5 * MINUTE_IN_SECONDS);
    return $assets;
}

add_action('template_redirect', static function () {
    $request_path = wp_parse_url($_SERVER['REQUEST_URI'] ?? '', PHP_URL_PATH);
    $platform = [
        '/download/macos' => 'macos',
        '/download/macos/' => 'macos',
        '/download/windows' => 'windows',
        '/download/windows/' => 'windows',
    ][$request_path] ?? null;
    if ($platform !== null) {
        $assets = beale_latest_desktop_assets();
        $url = $assets[$platform] ?? 'https://github.com/philo-groves/beale-mono/releases/latest';
        header('Cache-Control: no-store');
        wp_redirect($url, 302, 'Beale');
        exit;
    }

    if ($request_path !== '/i' && $request_path !== '/i/') {
        return;
    }

    $installer = __DIR__ . '/install.mjs';
    if (!is_readable($installer)) {
        status_header(503);
        exit;
    }

    status_header(200);
    header('Content-Type: text/plain; charset=utf-8');
    header('X-Content-Type-Options: nosniff');
    header('Cache-Control: no-store');
    header('Content-Length: ' . filesize($installer));
    readfile($installer);
    exit;
}, 0);
