// Phase 0 spike (grafida/grafida#82): C++23 + saucer v8 on Linux. Same tests as the wry spike.
#include <saucer/smartview.hpp>
#include <saucer/modules/desktop.hpp>
#include <picosha2.h>

#include <atomic>
#include <chrono>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <map>
#include <mutex>
#include <sstream>
#include <thread>

namespace fs = std::filesystem;
using saucer::scheme::response;

static std::string read_file(const fs::path &p)
{
    std::ifstream f(p, std::ios::binary);
    std::stringstream s;
    s << f.rdbuf();
    return s.str();
}

static response json(int status, std::string body)
{
    return {.data = saucer::stash::from_str(body), .mime = "application/json", .headers = {{"cache-control", "no-store"}}, .status = status};
}

coco::stray start(saucer::application *app)
{
    auto desktop = saucer::modules::desktop{app};
    auto window  = saucer::window::create(app).value();
    window->set_title("Shell spike (saucer)");
    window->set_size({1280, 860});

    auto webview = saucer::smartview::create({.window = window}).value();
    webview.set_dev_tools(std::getenv("GRAFIDA_DEBUG") != nullptr);
    webview.set_context_menu(true);

    static const std::string index = read_file(INDEX_HTML);
    static std::mutex mtx;
    static std::map<std::string, fs::path> tokens;
    static std::atomic<int> seq{1};

    webview.on<saucer::webview::event::navigate>([](const saucer::navigation &nav) {
        auto u = nav.url().string();
        if (u.starts_with("boson://app/")) return saucer::policy::allow;
        std::cerr << "[nav] blocked " << u << (nav.new_window() ? " (new window)" : "") << '\n';
        return saucer::policy::block;
    });

    webview.handle_scheme("boson", [app, &desktop](saucer::scheme::request req, saucer::scheme::executor exec) {
        const auto url    = req.url();
        const auto path   = url.path().string();
        const auto method = req.method();
        auto body         = req.content();
        std::cerr << "[req] " << method << ' ' << url.string() << " body=" << body.size() << '\n';

        if (method == "GET" && path == "/")
            return exec.resolve({.data = saucer::stash::view_str(index), .mime = "text/html"});

        if (method == "POST" && path == "/api/echo") {
            std::string hex;
            picosha2::hash256_hex_string(body.data(), body.data() + body.size(), hex);
            return exec.resolve(json(200, std::format(R"({{"bytes":{},"sha256":"{}"}})", body.size(), hex)));
        }
        if (method == "GET" && path == "/api/headers") {
            // std::map: a repeated response header cannot be expressed at all.
            return exec.resolve({.data = saucer::stash::from_str("{}"), .mime = "application/json",
                                 .headers = {{"cache-control", "no-store"}, {"x-multi", "one"}}});
        }
        if (method == "GET" && path == "/api/slow") {
            std::thread([app, exec] {
                std::this_thread::sleep_for(std::chrono::milliseconds(3000));
                app->post([exec] { exec.resolve(json(200, R"({"slept":3000})")); });
            }).detach();
            return;
        }
        if (method == "POST" && path == "/_native/dialog/open-file") {
            fs::path picked;
            if (const char *ap = std::getenv("SPIKE_AUTOPICK")) picked = ap;
            else {
                auto r = desktop.pick<saucer::modules::picker::type::file>({});
                if (!r) return exec.resolve(json(200, R"({"cancelled":true})"));
                picked = *r;
            }
            auto token = std::format("t{}-{}", seq++, std::hash<std::string>{}(picked.string() + std::to_string(std::chrono::steady_clock::now().time_since_epoch().count())));
            { std::lock_guard l(mtx); tokens[token] = picked; }
            return exec.resolve(json(200, std::format(R"({{"files":[{{"token":"{}","name":"{}","size":{}}}]}})",
                                                     token, picked.filename().string(), fs::file_size(picked))));
        }
        if (method == "POST" && path == "/api/read-file") {
            auto headers  = req.headers();
            bool injected = headers.erase("x-shell-file-path") + headers.erase("X-Shell-File-Path") > 0;
            fs::path p;
            for (auto &[k, v] : headers) {
                std::string lk = k; std::ranges::transform(lk, lk.begin(), ::tolower);
                if (lk == "x-shell-file") { std::lock_guard l(mtx); if (tokens.contains(v)) p = tokens[v]; }
            }
            if (p.empty()) return exec.resolve(json(400, std::format(R"({{"error":"no_file","strippedInjectedPath":{}}})", injected)));
            auto data = read_file(p);
            std::string hex;
            picosha2::hash256_hex_string(data.begin(), data.end(), hex);
            return exec.resolve(json(200, std::format(R"({{"path":"{}","bytes":{},"sha256":"{}"}})", p.string(), data.size(), hex)));
        }
        if (method == "POST" && path == "/report") {
            std::cout << "REPORT " << std::string(reinterpret_cast<const char *>(body.data()), body.size()) << std::endl;
            return exec.resolve(json(200, R"({"ok":true})"));
        }
        exec.resolve(json(404, R"({"error":"not_found"})"));
    });

    webview.set_url("boson://app/");
    window->show();
    co_await app->finish();
}

int main()
{
    saucer::webview::register_scheme("boson");
    return saucer::application::create({.id = "app.grafida.spike"})->run(start);
}
