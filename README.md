# byrdocs-check

## Usage

First, compile the binaries of PDFium and place them in the `./lib` directory with the appropriate filename, such as `libpdfium.xxx`.

Then

```sh
cargo run --bin check-pr -d "./path_you_want_to_check" ...
```

Or

```sh
cargo run --bin check-cmt -d "./path_you_want_to_check" ...
```

check-pr: Check the PRs in the specified directory.(Check format)

check-cmt: Check the comments in the specified directory.(Upload cover and metadata)

`upload-metadata` supports an optional environment variable `SITEMAP_MIN_LASTMOD`.
When set, any sitemap `lastmod` earlier than this value will be replaced by it.
Supported formats: `YYYY-MM-DD` or RFC3339, for example `2024-01-01` or `2024-01-01T00:00:00+08:00`.

Homepage and `/about` sitemap `lastmod` are fetched from GitHub.
You can override the target repository with `BYRDOCS_SITE_REPO_OWNER` and `BYRDOCS_SITE_REPO_NAME`; both default to `byrdocs`.

`check-format` supports an optional `IGNORE_ISBN_CHECK_LIST` environment variable for ISBNs shared by distinct files. Use `<ISBN>:<MD5>[,<MD5>...][;<ISBN>:<MD5>[,<MD5>...]]...`; a trailing semicolon is allowed. For example:

```sh
IGNORE_ISBN_CHECK_LIST="9787030822918:9c2b102e98f95fdf8fa53cfe98490ed1,a688a38c921cfe3b940175a9b047e941;"
```

For a duplicate ISBN, the error is suppressed only when every file's MD5 is listed for that ISBN. MD5 values are case-insensitive; invalid ISBNs or MD5 values make the configuration invalid.
