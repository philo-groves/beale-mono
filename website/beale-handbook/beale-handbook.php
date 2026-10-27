<?php
/**
 * Plugin Name: Beale Handbook
 * Description: Serves the Beale product handbook at /docs/.
 * Version: 1.0.1
 * Requires PHP: 7.4
 */

if (!defined('ABSPATH')) {
    exit;
}

function beale_handbook_catalog() {
    static $catalog = null;
    if ($catalog !== null) {
        return $catalog;
    }
    $source = is_readable(__DIR__ . '/content.json') ? file_get_contents(__DIR__ . '/content.json') : false;
    $decoded = $source === false ? null : json_decode($source, true);
    $catalog = is_array($decoded) ? $decoded : [];
    $order = ['start-here', 'architecture', 'ways-to-work', 'workspaces', 'information', 'agent-settings', 'other-features'];
    usort($catalog, static function ($left, $right) use ($order) {
        return array_search($left['slug'], $order, true) <=> array_search($right['slug'], $order, true);
    });
    return $catalog;
}

function beale_handbook_url($section, $page = null) {
    $path = '/docs/' . $section . '/';
    if ($page !== null) {
        $path .= $page . '/';
    }
    return home_url($path);
}

function beale_handbook_find($sections, $section_slug, $page_slug) {
    foreach ($sections as $section) {
        if ($section['slug'] !== $section_slug) {
            continue;
        }
        if ($page_slug === null) {
            return [$section, null];
        }
        foreach ($section['pages'] as $page) {
            if ($page['slug'] === $page_slug) {
                return [$section, $page];
            }
        }
    }
    return [null, null];
}

function beale_handbook_render_blocks($blocks) {
    foreach ($blocks as $block) {
        if (!empty($block['heading'])) {
            echo '<h2>' . esc_html($block['heading']) . '</h2>';
        }
        foreach ($block['paragraphs'] ?? [] as $paragraph) {
            echo '<p>' . esc_html($paragraph) . '</p>';
        }
        if (!empty($block['steps'])) {
            echo '<ol>';
            foreach ($block['steps'] as $step) {
                echo '<li>' . esc_html($step) . '</li>';
            }
            echo '</ol>';
        }
        if (!empty($block['bullets'])) {
            echo '<ul>';
            foreach ($block['bullets'] as $item) {
                echo '<li>' . esc_html($item) . '</li>';
            }
            echo '</ul>';
        }
        if (!empty($block['code'])) {
            echo '<pre><code>' . esc_html($block['code']) . '</code></pre>';
        }
        if (!empty($block['note'])) {
            echo '<aside class="bh-note">' . esc_html($block['note']) . '</aside>';
        }
        foreach ($block['links'] ?? [] as $link) {
            echo '<p><a class="bh-text-link" href="' . esc_url($link['url']) . '">' . esc_html($link['label']) . ' →</a></p>';
        }
    }
}

add_action('template_redirect', static function () {
    $path = wp_parse_url($_SERVER['REQUEST_URI'] ?? '', PHP_URL_PATH);
    if (!is_string($path) || !preg_match('~^/docs(?:/([a-z0-9-]+)(?:/([a-z0-9-]+))?)?/?$~D', $path, $matches)) {
        return;
    }
    $sections = beale_handbook_catalog();
    if (!$sections) {
        status_header(503);
        exit;
    }
    $section_slug = $matches[1] ?? null;
    $page_slug = $matches[2] ?? null;
    [$section, $page] = $section_slug === null
        ? [null, null]
        : beale_handbook_find($sections, $section_slug, $page_slug);
    if ($section_slug !== null && $section === null) {
        return;
    }
    $title = $page['title'] ?? ($section['title'] ?? 'Documentation');
    $description = $page['summary'] ?? ($section['summary'] ?? 'Use Beale for authorized research.');
    $canonical = $page !== null
        ? beale_handbook_url($section_slug, $page_slug)
        : ($section !== null ? beale_handbook_url($section_slug) : home_url('/docs/'));
    // Leaf handbook routes are served here without WordPress rewrite entries.
    // Clear the query's 404 flag so wp_head does not emit noindex metadata.
    global $wp_query;
    if ($wp_query instanceof WP_Query) {
        $wp_query->is_404 = false;
    }
    status_header(200);
    add_filter('pre_get_document_title', static function () use ($title) {
        return $title . ' – Beale Docs';
    });
    ?><!doctype html>
    <html <?php language_attributes(); ?>>
    <head>
        <meta charset="<?php bloginfo('charset'); ?>">
        <meta name="viewport" content="width=device-width, initial-scale=1">
        <meta name="description" content="<?php echo esc_attr($description); ?>">
        <link rel="canonical" href="<?php echo esc_url($canonical); ?>">
        <?php wp_head(); ?>
        <link rel="stylesheet" href="<?php echo esc_url(plugins_url('handbook.css', __FILE__)); ?>?v=1.0.0">
    </head>
    <body <?php body_class('beale-handbook'); ?>>
    <?php wp_body_open(); ?>
    <a class="bh-skip" href="#bh-content">Skip to content</a>
    <header class="bh-header"><div class="bh-container bh-header-inner">
        <a class="bh-brand" href="<?php echo esc_url(home_url('/')); ?>" aria-label="Beale home">beale<span>.</span></a>
        <nav aria-label="Site navigation"><a href="<?php echo esc_url(home_url('/blog/')); ?>">Blog</a><a aria-current="page" href="<?php echo esc_url(home_url('/docs/')); ?>">Docs</a><a href="<?php echo esc_url(home_url('/downloads/')); ?>">Downloads</a></nav>
        <a class="bh-source" href="https://github.com/philo-groves/beale-mono">Explore the source ↗</a>
    </div></header>
    <div class="bh-container bh-layout">
        <aside class="bh-sidebar" aria-label="Documentation navigation">
            <a class="bh-sidebar-title" href="<?php echo esc_url(home_url('/docs/')); ?>">The Beale Handbook</a>
            <?php foreach ($sections as $nav_section): ?>
                <div class="bh-nav-section">
                    <a class="bh-nav-heading" href="<?php echo esc_url(beale_handbook_url($nav_section['slug'])); ?>"><?php echo esc_html($nav_section['title']); ?></a>
                    <?php foreach ($nav_section['pages'] as $nav_page): ?>
                        <a class="bh-nav-page" <?php echo $section_slug === $nav_section['slug'] && $page_slug === $nav_page['slug'] ? 'aria-current="page"' : ''; ?> href="<?php echo esc_url(beale_handbook_url($nav_section['slug'], $nav_page['slug'])); ?>"><?php echo esc_html($nav_page['title']); ?></a>
                    <?php endforeach; ?>
                </div>
            <?php endforeach; ?>
        </aside>
        <main class="bh-main" id="bh-content">
            <?php if ($section !== null): ?>
                <nav class="bh-breadcrumb" aria-label="Breadcrumb"><a href="<?php echo esc_url(home_url('/docs/')); ?>">Docs</a><span>/</span><?php if ($page !== null): ?><a href="<?php echo esc_url(beale_handbook_url($section_slug)); ?>"><?php echo esc_html($section['title']); ?></a><span>/</span><?php endif; ?><span><?php echo esc_html($title); ?></span></nav>
            <?php endif; ?>
            <p class="bh-eyebrow">THE BEALE HANDBOOK</p>
            <h1><?php echo esc_html($title); ?></h1>
            <p class="bh-lede"><?php echo esc_html($description); ?></p>
            <?php if ($page !== null): ?>
                <article class="bh-article"><?php beale_handbook_render_blocks($page['blocks']); ?></article>
                <div class="bh-next"><a href="<?php echo esc_url(beale_handbook_url($section_slug)); ?>">More in <?php echo esc_html($section['title']); ?> →</a></div>
            <?php elseif ($section !== null): ?>
                <div class="bh-cards"><?php foreach ($section['pages'] as $item): ?><a class="bh-card" href="<?php echo esc_url(beale_handbook_url($section_slug, $item['slug'])); ?>"><strong><?php echo esc_html($item['title']); ?></strong><span><?php echo esc_html($item['summary']); ?></span><em>Read guide →</em></a><?php endforeach; ?></div>
            <?php else: ?>
                <div class="bh-callout"><strong>Start here</strong><p>Beale Desktop is available for macOS and Windows. The iOS agent is in development and is not available on the App Store.</p><a href="<?php echo esc_url(home_url('/downloads/')); ?>">Get Beale Desktop →</a></div>
                <?php foreach ($sections as $group): ?><section class="bh-index-section"><h2><a href="<?php echo esc_url(beale_handbook_url($group['slug'])); ?>"><?php echo esc_html($group['title']); ?> →</a></h2><p><?php echo esc_html($group['summary']); ?></p><div class="bh-index-links"><?php foreach ($group['pages'] as $item): ?><a href="<?php echo esc_url(beale_handbook_url($group['slug'], $item['slug'])); ?>"><?php echo esc_html($item['title']); ?></a><?php endforeach; ?></div></section><?php endforeach; ?>
            <?php endif; ?>
        </main>
    </div>
    <footer class="bh-footer"><div class="bh-container"><a href="<?php echo esc_url(home_url('/')); ?>">beale<span>.</span></a><p>Human-steered. Evidence-led. Open source under MIT.</p><nav aria-label="Footer navigation"><a href="<?php echo esc_url(home_url('/downloads/')); ?>">Downloads</a><a href="https://github.com/philo-groves/beale-mono">GitHub ↗</a></nav></div></footer>
    <?php wp_footer(); ?>
    </body></html><?php
    exit;
}, 0);
