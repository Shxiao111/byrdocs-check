use rusoto_core::HttpClient;
use rusoto_s3::S3;
use std::collections::HashSet;
use std::path::Path;
use tokio::{
    self,
    io::{AsyncReadExt, BufReader},
};

use byrdocs_check::{get_env, get_optional_env, metadata::*};

struct Input {
    metadata_dir: String,
    r2_endpoint: String,
    r2_access_key_id: String,
    r2_secret_access_key: String,
    r2_file_bucket: String,
    byrdocs_site_url: String,
    byrdocs_site_token: String,
}

impl Input {
    fn new() -> Self {
        Input {
            metadata_dir: get_env("METADATA_DIR"),
            r2_endpoint: format!(
                "https://{}.r2.cloudflarestorage.com",
                get_env("R2_ACCOUNT_ID"),
            ),
            r2_access_key_id: get_env("R2_ACCESS_KEY_ID"),
            r2_secret_access_key: get_env("R2_SECRET_ACCESS_KEY"),
            r2_file_bucket: get_env("R2_FILE_BUCKET"),
            byrdocs_site_url: get_env("BYRDOCS_SITE_URL"),
            byrdocs_site_token: get_env("BYRDOCS_SITE_TOKEN"),
        }
    }
}

#[derive(serde::Deserialize, Debug)]
#[allow(dead_code)]
struct ApiResult {
    success: bool,
    files: Vec<TempFiles>,
}

#[derive(serde::Deserialize, Debug)]
#[allow(dead_code)]
struct TempFiles {
    id: u64,
    #[serde(rename = "createdAt")]
    created_at: String,
    #[serde(rename = "fileName")]
    file_name: String,
    #[serde(rename = "fileSize")]
    file_size: Option<u64>,
    uploader: String,
    #[serde(rename = "uploadTime")]
    upload_time: Option<String>,
    status: Status,
    #[serde(rename = "errorMessage")]
    error_message: Option<String>,
}

#[derive(serde::Deserialize, Debug)]
enum Status {
    Published,
    Pending,
    Timeout,
    Expired,
    Error,
    Uploaded,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let input = Input::new();
    let mut isbn_check = IsbnCheck::from_env()?;

    let end_point = input.r2_endpoint;
    let http_client = HttpClient::new()?;
    let credentials = rusoto_core::credential::StaticProvider::new_minimal(
        input.r2_access_key_id,
        input.r2_secret_access_key,
    );
    let region = rusoto_core::Region::Custom {
        name: "auto".to_owned(),
        endpoint: end_point,
    };
    let s3_client = rusoto_s3::S3Client::new_with(http_client, credentials, region);
    let s3_file_list = list_all_objects(&s3_client, &input.r2_file_bucket).await;

    let backend_client = reqwest::Client::new();
    let api_result = backend_client
        .get(format!("{}/api/file/notPublished", input.byrdocs_site_url))
        .bearer_auth(input.byrdocs_site_token)
        .send()
        .await
        .unwrap()
        .json::<ApiResult>()
        .await
        .unwrap();

    let mut success_book = 0;
    let mut success_test = 0;
    let mut success_doc = 0;
    let mut total = 0;
    let dir_path = Path::new(&input.metadata_dir);

    for entry in dir_path.read_dir()? {
        total += 1;
        let entry = entry?;
        let file_name = entry.file_name();
        if !file_name.to_str().unwrap().ends_with(".yml") {
            if file_name.to_str().unwrap().ends_with(".yaml") {
                println!("{:?}:\n  请将.yaml改为.yml", file_name);
            } else {
                println!("{:?}:\n  请检查文件名后缀, 只能为\".yml\"", file_name);
            }
            continue;
        }
        let path = entry.path();
        if path.is_file() {
            let file = std::fs::File::open(&path)?;
            let metadata: MetaData = match serde_yaml::from_reader(file) {
                Ok(metadata) => metadata,
                Err(e) => {
                    println!("{:?}:\n  格式错误: {:?}", path.file_name().unwrap(), e);
                    continue;
                }
            };

            if let Err(e) = check(
                &metadata,
                &metadata.id,
                &mut isbn_check,
                &s3_file_list,
                &path,
                &api_result,
                &input.r2_file_bucket,
                &s3_client,
                &input.byrdocs_site_url,
            )
            .await
            {
                println!("{:?}:\n  {:?}", path.file_name().unwrap(), e);
                continue;
            };

            match metadata.data {
                Data::Test(_) => {
                    success_test += 1;
                }
                Data::Book(_) => {
                    success_book += 1;
                }
                Data::Doc(_) => {
                    success_doc += 1;
                }
            }
        }
    }
    println!(
        "Total: {}, Success: {}, Book: {}, Test: {}, Doc: {}",
        total,
        success_book + success_test + success_doc,
        success_book,
        success_test,
        success_doc
    );
    if total != success_book + success_test + success_doc {
        return Err(anyhow::anyhow!(
            "自动检查不通过，请按照上述报错信息修改您的元信息"
        ));
    } else {
        println!("自动检查通过，我们正在审核您的文件，请耐心等待");
    }
    Ok(())
}

async fn list_all_objects(client: &rusoto_s3::S3Client, bucket_name: &str) -> Vec<String> {
    let mut continuation_token: Option<String> = None;
    let mut s3_file_list = Vec::new();

    loop {
        let request = rusoto_s3::ListObjectsV2Request {
            bucket: bucket_name.to_string(),
            continuation_token: continuation_token.clone(),
            ..Default::default()
        };

        match client.list_objects_v2(request).await {
            Ok(output) => {
                if let Some(contents) = output.contents {
                    s3_file_list.extend(
                        contents
                            .iter()
                            .map(|item| item.key.clone().unwrap())
                            .collect::<Vec<_>>(),
                    )
                }

                if let Some(next_continuation_token) = output.next_continuation_token {
                    continuation_token = Some(next_continuation_token);
                } else {
                    break;
                }
            }
            Err(rusoto_core::RusotoError::Unknown(resp)) => {
                println!("Error: {}", resp.status);
                break;
            }
            Err(e) => {
                println!("Error: {:?}", e);
                break;
            }
        }
    }
    s3_file_list
}

#[derive(Default)]
struct IsbnCheck {
    allowed_duplicates: Vec<(isbn::Isbn13, HashSet<String>)>,
    seen: Vec<(isbn::Isbn13, HashSet<String>)>,
}

impl IsbnCheck {
    fn from_env() -> anyhow::Result<Self> {
        let Some(value) = get_optional_env("IGNORE_ISBN_CHECK_LIST") else {
            return Ok(Self::default());
        };
        Self::parse(&value)
    }

    fn parse(value: &str) -> anyhow::Result<Self> {
        let mut check = Self::default();
        let entries: Vec<_> = value.split(';').collect();
        for (entry_index, entry) in entries.iter().enumerate() {
            let entry = entry.trim();
            if entry.is_empty() {
                if entry_index + 1 == entries.len() {
                    continue;
                }
                anyhow::bail!(
                    "Invalid IGNORE_ISBN_CHECK_LIST: empty entry at position {}",
                    entry_index + 1
                );
            }

            let (isbn_value, md5_values) = entry.split_once(':').ok_or_else(|| {
                anyhow::anyhow!(
                    "Invalid IGNORE_ISBN_CHECK_LIST entry {:?}: expected <ISBN>:<MD5>[,<MD5>...]",
                    entry
                )
            })?;
            if md5_values.contains(':') {
                anyhow::bail!(
                    "Invalid IGNORE_ISBN_CHECK_LIST entry {:?}: expected one ':'",
                    entry
                );
            }

            let isbn = isbn_value.trim().parse::<isbn::Isbn13>().map_err(|error| {
                anyhow::anyhow!(
                    "Invalid ISBN in IGNORE_ISBN_CHECK_LIST entry {:?}: {}",
                    entry,
                    error
                )
            })?;
            let hashes = check.hashes_for_isbn_mut(isbn);
            for hash in md5_values.split(',') {
                let hash = hash.trim();
                if hash.len() != 32 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    anyhow::bail!(
                        "Invalid MD5 {:?} in IGNORE_ISBN_CHECK_LIST entry {:?}: expected 32 hexadecimal characters",
                        hash,
                        entry
                    );
                }
                hashes.insert(hash.to_ascii_lowercase());
            }
        }

        Ok(check)
    }

    fn hashes_for_isbn_mut(&mut self, isbn: isbn::Isbn13) -> &mut HashSet<String> {
        if let Some(index) = self
            .allowed_duplicates
            .iter()
            .position(|(existing_isbn, _)| existing_isbn == &isbn)
        {
            return &mut self.allowed_duplicates[index].1;
        }
        self.allowed_duplicates.push((isbn, HashSet::new()));
        &mut self.allowed_duplicates.last_mut().unwrap().1
    }

    fn record(&mut self, isbn: isbn::Isbn13, md5: &str) -> bool {
        let md5 = md5.to_ascii_lowercase();
        let seen_index = self
            .seen
            .iter()
            .position(|(existing_isbn, _)| existing_isbn == &isbn);
        let allowed_index = self
            .allowed_duplicates
            .iter()
            .position(|(existing_isbn, _)| existing_isbn == &isbn);
        let allowed = allowed_index.map(|index| &self.allowed_duplicates[index].1);

        if let Some(index) = seen_index {
            let seen_hashes = &mut self.seen[index].1;
            let is_new_hash = !seen_hashes.contains(&md5);
            let duplicate_is_allowed = is_new_hash
                && allowed.is_some_and(|allowed| {
                    allowed.contains(&md5) && seen_hashes.iter().all(|hash| allowed.contains(hash))
                });
            seen_hashes.insert(md5);
            !is_new_hash || duplicate_is_allowed
        } else {
            self.seen.push((isbn, HashSet::from([md5])));
            true
        }
    }
}

async fn check(
    metadata: &MetaData,
    md5: &str,
    isbn_check: &mut IsbnCheck,
    s3_file_list: &Vec<String>,
    path: &Path,
    api_result: &ApiResult,
    r2_file_bucket: &str,
    s3_client: &rusoto_s3::S3Client,
    byrdocs_site_url: &str,
) -> anyhow::Result<()> {
    let mut errors = Vec::new();

    let data = &metadata.data;
    if let Err(e) = match data {
        Data::Test(test) => check_test(test),
        Data::Book(book) => check_book(book, md5, isbn_check),
        Data::Doc(doc) => check_doc(doc),
    } {
        errors.push(e);
    }

    let url_regex = regex::Regex::new(&format!(
        r"^{}/files/[a-fA-F0-9]{{32}}\.(pdf|zip)$",
        regex::escape(&byrdocs_site_url),
    ))
    .unwrap();

    if !s3_file_list.contains(&metadata.url[metadata.url.len() - 36..].to_string()) {
        errors.push(anyhow::anyhow!("请检查文件是否上传"));
    }

    if !url_regex.is_match(&metadata.url) {
        errors.push(anyhow::anyhow!("请检查url是否填写正确"));
    }

    if !metadata.url.contains(metadata.id.as_str()) {
        errors.push(anyhow::anyhow!("请检查url中文件名与id是否匹配"));
    }

    if !(path.file_name().unwrap().to_str().unwrap() == format!("{}.yml", metadata.id)) {
        errors.push(anyhow::anyhow!("请检查文件名是否与id匹配"));
    }

    let mut unmatched = false;
    for temp_file in &api_result.files {
        if temp_file.file_name.as_str() == format!("{}.pdf", metadata.id)
            || temp_file.file_name.as_str() == format!("{}.zip", metadata.id)
        {
            match temp_file.status {
                Status::Expired => {
                    errors.push(anyhow::anyhow!("文件已过期，请重新上传"));
                }
                Status::Timeout | Status::Pending => {
                    errors.push(anyhow::anyhow!("文件未上传成功，请重新上传"));
                }
                Status::Error => {
                    errors.push(anyhow::anyhow!("api 未知错误"));
                }
                _ => {
                    let request = rusoto_s3::GetObjectRequest {
                        bucket: r2_file_bucket.to_string(),
                        key: metadata.url[metadata.url.len() - 36..].to_string(),
                        ..Default::default()
                    };
                    let file = s3_client.get_object(request).await?.body.unwrap();
                    let mut reader = BufReader::new(file.into_async_read());
                    let mut context = md5::Context::new();
                    let mut buffer = [0; 8192];
                    loop {
                        let bytes_read = reader.read(&mut buffer).await?;
                        if bytes_read == 0 {
                            break;
                        }
                        context.consume(&buffer[..bytes_read]);
                    }
                    let md5 = context.compute();
                    if !(format!("{:x}", md5) == metadata.id) {
                        unmatched = true;
                    }
                }
            }
            break;
        }
    }
    if unmatched {
        errors.push(anyhow::anyhow!("md5不匹配"));
    }

    if errors.is_empty() {
        Ok(())
    } else {
        let error_messages: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
        Err(anyhow::anyhow!(error_messages.join("\n")))
    }
}

fn check_book(book: &Book, md5: &str, isbn_check: &mut IsbnCheck) -> anyhow::Result<()> {
    let mut errors = Vec::new();
    if book.authors.len() == 0 {
        errors.push(anyhow::anyhow!("应当至少有一个作者"));
    }
    let year_regex = regex::Regex::new(r"^\d{4}$").unwrap();
    if let Some(year) = &book.publish_year {
        if !year_regex.is_match(year) {
            errors.push(anyhow::anyhow!("请检查出版年份"));
        }
    }
    book.isbn
        .iter()
        .for_each(|isbn| match isbn.parse::<isbn::Isbn13>() {
            Ok(isbn) => {
                if !isbn_check.record(isbn, md5) {
                    errors.push(anyhow::anyhow!("重复的isbn. md5: {}", md5));
                }
            }
            Err(e) => {
                errors.push(anyhow::anyhow!("请检查isbn格式：{}", e));
            }
        });
    if book.filetype != "pdf" {
        errors.push(anyhow::anyhow!("请检查filetype，只能为pdf"));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        let error_messages: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
        Err(anyhow::anyhow!(error_messages.join("\n")))
    }
}

fn check_test(test: &Test) -> anyhow::Result<()> {
    let mut errors = Vec::new();
    if let Some(test) = &test.course.type_ {
        if !["本科", "研究生"].contains(&test.as_str()) {
            errors.push(anyhow::anyhow!(
                "请检查course type，只能为\"本科\"或\"研究生\""
            ));
        }
    }
    if let Some(stage) = &test.time.stage {
        if !["期中", "期末"].contains(&stage.as_str()) {
            errors.push(anyhow::anyhow!("请检查stage，只能为\"期中\"或\"期末\""));
        }
    }
    if let Some(semester) = &test.time.semester {
        if !["First", "Second"].contains(&semester.as_str()) {
            errors.push(anyhow::anyhow!(
                "请检查semester，只能为\"First\"或\"Second\""
            ));
        }
    }
    for content in &test.content {
        match content.as_str() {
            "原题" => (),
            "答案" => (),
            _ => errors.push(anyhow::anyhow!(
                "错误的content，content只能为\"原题\"或\"答案\""
            )),
        }
    }
    if let Some(colleges) = &test.college {
        if colleges.contains(&"".to_string()) {
            errors.push(anyhow::anyhow!("college不能存在空字符串"));
        }
    }
    if let (Ok(start), Ok(end)) = (test.time.start.parse::<u32>(), test.time.end.parse::<u32>()) {
        if !(start == end || start + 1 == end) {
            errors.push(anyhow::anyhow!("请检查时间"));
        }
    } else {
        errors.push(anyhow::anyhow!("时间格式不正确"));
    }
    if !["pdf", "zip"].contains(&test.filetype.as_str()) {
        errors.push(anyhow::anyhow!("请检查filetype，只能为pdf或zip"));
    }
    if let Some(source) = &test.external_source {
        if source != "bump" {
            errors.push(anyhow::anyhow!("请检查external_source，只能为bump"));
        }
    }
    if test.content.len() == 0 {
        errors.push(anyhow::anyhow!("content不能为空"));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        let error_messages: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
        Err(anyhow::anyhow!(error_messages.join("\n")))
    }
}

fn check_doc(doc: &Doc) -> anyhow::Result<()> {
    let mut errors = Vec::new();
    if &doc.course.len() == &0 {
        errors.push(anyhow::anyhow!("course不能为空"));
    }
    for course in &doc.course {
        if let Some(test) = &course.type_ {
            if !["本科", "研究生"].contains(&test.as_str()) {
                errors.push(anyhow::anyhow!(
                    "请检查course type，只能为\"本科\"或\"研究生\""
                ));
            }
        }
    }
    if &doc.content.len() == &0 {
        errors.push(anyhow::anyhow!("content不能为空"));
    }
    for content in &doc.content {
        match content.as_str() {
            "思维导图" => (),
            "题库" => (),
            "答案" => (),
            "知识点" => (),
            "课件" => (),
            _ => errors.push(anyhow::anyhow!(
                r#"错误的content，content只能为"思维导图"、"题库"、"答案"、"知识点"或"课件""#
            )),
        }
    }
    if doc.filetype != "pdf" && doc.filetype != "zip" {
        errors.push(anyhow::anyhow!("请检查filetype，只能为pdf或zip"));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        let error_messages: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
        Err(anyhow::anyhow!(error_messages.join("\n")))
    }
}

#[cfg(test)]
mod test {
    #[test]
    fn allows_only_collisions_whose_hashes_are_all_listed() {
        let config =
            "9787030822918:9c2b102e98f95fdf8fa53cfe98490ed1,a688a38c921cfe3b940175a9b047e941;";
        let mut forward = super::IsbnCheck::parse(config).unwrap();
        let isbn = "9787030822918".parse().unwrap();
        assert!(forward.record(isbn, "9c2b102e98f95fdf8fa53cfe98490ed1"));
        assert!(forward.record(isbn, "a688a38c921cfe3b940175a9b047e941"));
        assert!(!forward.record(isbn, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"));

        let mut reverse = super::IsbnCheck::parse(config).unwrap();
        assert!(reverse.record(isbn, "a688a38c921cfe3b940175a9b047e941"));
        assert!(reverse.record(isbn, "9c2b102e98f95fdf8fa53cfe98490ed1"));
        assert!(!reverse.record(isbn, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"));

        let mut unlisted_first = super::IsbnCheck::parse(config).unwrap();
        assert!(unlisted_first.record(isbn, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"));
        assert!(!unlisted_first.record(isbn, "9c2b102e98f95fdf8fa53cfe98490ed1"));
    }

    #[test]
    fn parses_ignore_isbn_check_list_and_validates_md5s() {
        let config =
            super::IsbnCheck::parse(" 9787030822918 : 9C2B102E98F95FDF8FA53CFE98490ED1 ;").unwrap();
        let allowed = &config.allowed_duplicates[0].1;
        assert!(allowed.contains("9c2b102e98f95fdf8fa53cfe98490ed1"));
        assert!(super::IsbnCheck::parse("9787030822918:not-an-md5").is_err());
        assert!(super::IsbnCheck::parse("9787030822918:").is_err());
    }

    #[test]
    fn validates_exam_filetype_and_external_source() {
        let mut paper: super::Test = serde_yaml::from_str(
            r#"
course:
  name: 数学分析（上）
time:
  start: '2025'
  end: '2026'
  semester: First
  stage: 期中
filetype: pdf
external_source: bump
content: [原题, 答案]
"#,
        )
        .unwrap();
        for (filetype, source, valid) in [
            ("pdf", Some("bump"), true),
            ("pdf", None, true),
            ("zip", None, true),
            ("bump", None, false),
            ("pdf", Some("other"), false),
        ] {
            paper.filetype = filetype.to_string();
            paper.external_source = source.map(str::to_owned);
            assert_eq!(
                super::check_test(&paper).is_ok(),
                valid,
                "filetype={filetype}, external_source={source:?}"
            );
        }
        paper.filetype = "pdf".to_owned();
        paper.external_source = Some("bump".to_owned());
        paper.content = vec!["课件".to_string()];
        assert!(super::check_test(&paper).is_err());
    }

    #[test]
    fn test_isbn() {
        let isbn = "978-7-111-40772-1";
        assert_eq!(
            isbn.parse::<isbn::Isbn13>(),
            Err(isbn::IsbnError::InvalidDigit)
        );
        let isbn = "978-7-111-40772-0";
        assert_eq!(
            isbn.parse::<isbn::Isbn13>(),
            Ok(isbn::Isbn13::new([9, 7, 8, 7, 1, 1, 1, 4, 0, 7, 7, 2, 0]).unwrap())
        );
        let isbn = "9787111407720";
        assert_eq!(
            isbn.parse::<isbn::Isbn13>(),
            Ok(isbn::Isbn13::new([9, 7, 8, 7, 1, 1, 1, 4, 0, 7, 7, 2, 0]).unwrap())
        );
        assert_eq!(
            "978-7-111-40772-0",
            isbn.parse::<isbn::Isbn13>()
                .unwrap()
                .hyphenate()
                .unwrap()
                .as_str()
        );
    }
}
