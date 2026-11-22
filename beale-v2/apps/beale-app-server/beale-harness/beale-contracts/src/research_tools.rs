//! Research tool IDs and labels shared by Fleet hosts and desktop clients.

pub struct ResearchTool {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub description: &'static str,
}

pub const CATALOG: &[ResearchTool] = &[
    ResearchTool {
        id: "ripgrep",
        name: "ripgrep",
        category: "Source",
        description: "Fast search across source and artifacts.",
    },
    ResearchTool {
        id: "opengrep",
        name: "Opengrep",
        category: "Source",
        description: "Rule-based source scanning.",
    },
    ResearchTool {
        id: "git",
        name: "Git",
        category: "Source",
        description: "Repository history and changes.",
    },
    ResearchTool {
        id: "subfinder",
        name: "subfinder",
        category: "Recon",
        description: "Discover subdomains using passive sources.",
    },
    ResearchTool {
        id: "nmap",
        name: "Nmap",
        category: "Recon",
        description: "Map reachable ports and network services.",
    },
    ResearchTool {
        id: "dig",
        name: "dig",
        category: "Recon",
        description: "Query DNS records and name servers.",
    },
    ResearchTool {
        id: "httpx",
        name: "httpx",
        category: "Web",
        description: "Probe HTTP services and collect response metadata.",
    },
    ResearchTool {
        id: "katana",
        name: "Katana",
        category: "Web",
        description: "Crawl websites to discover paths and endpoints.",
    },
    ResearchTool {
        id: "ffuf",
        name: "ffuf",
        category: "Web",
        description: "Fuzz web paths and request parameters.",
    },
    ResearchTool {
        id: "nuclei",
        name: "Nuclei",
        category: "Web",
        description: "Run template-based vulnerability checks.",
    },
    ResearchTool {
        id: "zap",
        name: "OWASP ZAP",
        category: "Web",
        description: "Intercept web traffic and assess application security.",
    },
    ResearchTool {
        id: "radare2",
        name: "radare2",
        category: "Binary",
        description: "Scriptable binary inspection and analysis.",
    },
    ResearchTool {
        id: "ghidra",
        name: "Ghidra",
        category: "Binary",
        description: "Disassembly and decompilation.",
    },
    ResearchTool {
        id: "llvm",
        name: "LLVM tools",
        category: "Binary",
        description: "Object, symbol, and disassembly utilities.",
    },
    ResearchTool {
        id: "otool",
        name: "otool",
        category: "Binary",
        description: "Inspect Mach-O headers, load commands, and disassembly.",
    },
    ResearchTool {
        id: "jtool2",
        name: "jtool2",
        category: "Binary",
        description: "Inspect and analyze Mach-O binaries and shared caches.",
    },
    ResearchTool {
        id: "jadx",
        name: "JADX",
        category: "Mobile",
        description: "Android APK and DEX decompilation.",
    },
    ResearchTool {
        id: "ipsw",
        name: "ipsw",
        category: "Mobile",
        description: "Apple firmware and binary inspection.",
    },
    ResearchTool {
        id: "python",
        name: "Python 3",
        category: "Runtimes",
        description: "Runtime for research scripts.",
    },
    ResearchTool {
        id: "java",
        name: "Java JDK",
        category: "Runtimes",
        description: "Runtime required by Java-based tools.",
    },
    ResearchTool {
        id: "node",
        name: "Node.js",
        category: "Runtimes",
        description: "Runtime for JavaScript research tooling.",
    },
];

pub fn available_on(id: &str, os: &str) -> bool {
    match id {
        "otool" => os == "macos",
        "jtool2" | "dig" => matches!(os, "macos" | "linux"),
        _ => true,
    }
}
