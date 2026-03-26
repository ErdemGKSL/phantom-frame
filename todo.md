enable websocket etc. should only be used for dynamic proxy fallback or non ssg mode tho.
Also implement https_port and http_port fields. // current naming is proxy_port, change its naming
currently it is named proxy_port.
make toml file support multiple servers at once.
[server.frontend] // ssg
bind_to = "*" // by default, * means bind to root, no need pattern matching here, bind as router to axum
[server.backend] // dynamic
bind_to = "/api" // this means basically axum router binding, where to bind, sort them so that shortest will be checked last, so that axum can see all of them, dont forget axum reads paths in reverse order compared to express js etc.
so that we can use this framework as reverse proxy.
https_port is provided, then certificate paths should be required too.
proxy_url naming should remain the same.