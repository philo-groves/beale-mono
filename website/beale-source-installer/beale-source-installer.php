<?php
/**
 * Plugin Name: Beale Source Installer
 * Description: Serves the Beale source installer at /i.
 * Version: 1.0.0
 * Requires PHP: 7.4
 */

if (!defined('ABSPATH')) {
    exit;
}

add_action('template_redirect', static function () {
    $request_path = wp_parse_url($_SERVER['REQUEST_URI'] ?? '', PHP_URL_PATH);
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
