use crate::error::{ApiError, ApiResult};
use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt};

async fn bounded(input: impl AsyncRead + Unpin) -> ApiResult<Vec<u8>> {
    let mut bytes = Vec::new();
    input
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(ApiError::internal)?;
    if bytes.len() > 1024 * 1024 {
        return Err(ApiError::bad("APICompat output exceeded 1 MiB"));
    }
    Ok(bytes)
}
async fn direction(dir: &Path, baseline: &str, target: &str, output: &str) -> ApiResult<Value> {
    let executable = std::env::var("APICOMPAT_PATH").unwrap_or_else(|_| "apicompat".into());
    let mut child=tokio::process::Command::new(executable)
        .args(["package",target,"--baseline-package",baseline,"--enable-rule-cannot-change-parameter-name","--generate-suppression-file","--suppression-output-file",output])
        .current_dir(dir).env("DOTNET_CLI_TELEMETRY_OPTOUT","1").env("DOTNET_NOLOGO","1")
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true)
        .spawn().map_err(|e| {tracing::warn!(error=%e,"APICompat could not start");ApiError::bad("APICompat is unavailable; install the documented tool and configure APICOMPAT_PATH")})?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ApiError::internal("Missing child stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ApiError::internal("Missing child stderr"))?;
    tokio::time::timeout(Duration::from_secs(60),async {
        let (status,out,err)=tokio::try_join!(async{child.wait().await.map_err(ApiError::internal)},bounded(stdout),bounded(stderr))?;
        if !status.success(){tracing::warn!(exit=?status.code(),"APICompat failed");return Err(ApiError::bad("APICompat could not complete analysis; no compatibility verdict is available"));}
        let path=dir.join(output);
        let mut findings=Vec::new();
        match tokio::fs::File::open(path).await {
            Ok(file)=>{
                let xml=bounded(file).await?;
                let xml=std::str::from_utf8(&xml).map_err(ApiError::internal)?;
                let doc=roxmltree::Document::parse(xml).map_err(ApiError::internal)?;
                for node in doc.descendants().filter(|n|n.has_tag_name("Suppression")) {
                    let field=|name|node.children().find(|n|n.has_tag_name(name)).and_then(|n|n.text()).unwrap_or_default();
                    findings.push(json!({"diagnosticId":field("DiagnosticId"),"target":field("Target"),"left":field("Left"),"right":field("Right")}));
                }
            }
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},
            Err(e)=>return Err(ApiError::internal(e)),
        }
        // Keep diagnostics as text as well: package-structure and assembly-resolution
        // failures may not be representable as API suppression entries.
        Ok(json!({"findings":findings,"output":String::from_utf8_lossy(&out),"stderr":String::from_utf8_lossy(&err)}))
    }).await.map_err(|_|ApiError::bad("APICompat exceeded its 60 second time limit"))?
}
pub(super) async fn compare(dir: &Path) -> ApiResult<Value> {
    let removed = direction(dir, "baseline.nupkg", "target.nupkg", "forward.xml").await?;
    let added = direction(dir, "target.nupkg", "baseline.nupkg", "reverse.xml").await?;
    Ok(
        json!({"engine":"Microsoft.DotNet.ApiCompat.Tool","baselineToTarget":removed,"targetToBaseline":added,"interpretation":"Forward findings identify potential breaks; reverse findings help identify additions. Diagnostic output also includes framework/asset consistency. No runtime compatibility guarantee."}),
    )
}
