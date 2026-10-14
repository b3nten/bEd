use std::{io, time::Duration};
pub fn run() {
    let mut mixed_document_opened = false;
    bedtime_remote::serve_with(io::stdin().lock(), io::stdout().lock(), |request| {
            if matches!(&request,bedtime_remote::Request::ReadFile{path,..} if path.ends_with("mixed.txt")) {
                if mixed_document_opened {
                    return Err(bedtime_remote::RemoteError::new(bedtime_remote::ErrorKind::PermissionDenied, "Fixture monitor read failed"));
                }
                mixed_document_opened = true;
            }
            if matches!(&request,bedtime_remote::Request::WriteFile{path,..} if path.ends_with("save-denied.txt")) {
                Err(bedtime_remote::RemoteError::new(bedtime_remote::ErrorKind::PermissionDenied, "Fixture save was denied"))
            } else if matches!(&request,bedtime_remote::Request::WriteFile{path,..} if path.ends_with("save-disconnected.txt")) {
                std::process::exit(0);
            } else if matches!(&request,bedtime_remote::Request::WriteFile{path,..} if path.ends_with("save-confirmed-disconnect.txt")) {
                let result = bedtime_remote::LocalBackend.call(request);
                std::thread::spawn(|| {
                    std::thread::sleep(Duration::from_millis(25));
                    std::process::exit(0);
                });
                result
            } else if matches!(&request,bedtime_remote::Request::ReadFile{path,..} if path.ends_with("wrong-response"))
                || matches!(&request,bedtime_remote::Request::WriteFile{path,..} if path.ends_with("save-wrong-response.txt")) {
                Ok(bedtime_remote::Response::Unit)
            } else {bedtime_remote::LocalBackend.call(request)}
        }).unwrap();
}
