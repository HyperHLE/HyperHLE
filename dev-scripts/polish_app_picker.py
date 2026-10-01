#!/usr/bin/env python3
"""In-place polish of src/environment/app_picker.rs — safe additive UI tweaks."""
from pathlib import Path

p = Path("src/environment/app_picker.rs")
t = p.read_text()
if "package_kind" in t:
    print("Already polished")
    raise SystemExit(0)

def repl(old, new, label):
    global t
    if old not in t:
        raise SystemExit(f"MISSING block: {label}")
    t = t.replace(old, new, 1)
    print("ok", label)

repl(
    """struct AppInfo {
    path: PathBuf,
    display_name: String,
    icon: Option<Image>,
    /// `NSString*`
    display_name_ns_string: Option<id>,
    /// `UIImage*`
    icon_ui_image: Option<id>,
}""",
    """struct AppInfo {
    path: PathBuf,
    display_name: String,
    bundle_id: Option<String>,
    version: Option<String>,
    package_kind: &'static str,
    icon: Option<Image>,
    display_name_ns_string: Option<id>,
    icon_ui_image: Option<id>,
}""",
    "AppInfo",
)

repl(
    'Err(format!("The {} directory couldn\'t be found. Check you\'re running touchHLE from the right directory.", apps_dir.display()))',
    'Err(format!("Apps folder missing.\\n\\nCreate this directory and add .app / .ipa files:\\n{}\\n\\nThen reopen HyperHLE.", apps_dir.display()))',
    "missing dir",
)

repl(
    '"No apps were found in the {} directory.",',
    '"No games yet.\\n\\nPut .app or .ipa files in:\\n{}\\n\\nThen reopen HyperHLE.",',
    "empty",
)

repl(
    """        // TODO: what if this crashes?
        let display_name = bundle.display_name().to_owned();

        let icon = match bundle.load_icon(&fs) {
            Ok(icon) => Some(icon),
            Err(e) => {
                log!("Warning: couldn't load icon for app bundle {}: {} (displaying placeholder instead)", app_path.display(), e);
                None
            }
        };

        apps.push(AppInfo {
            path: app_path,
            display_name,
            icon,
            display_name_ns_string: None,
            icon_ui_image: None,
        });""",
    """        // TODO: what if this crashes?
        let display_name = bundle.display_name().to_owned();
        let bundle_id = {
            let id = bundle.bundle_identifier();
            if id.is_empty() { None } else { Some(id.to_owned()) }
        };
        let version = {
            let v = bundle.bundle_version();
            if v.is_empty() { None } else { Some(v.to_owned()) }
        };
        let package_kind: &'static str =
            if app_path.extension() == Some(OsStr::new("ipa")) { "ipa" } else { "app" };

        let icon = match bundle.load_icon(&fs) {
            Ok(icon) => Some(icon),
            Err(e) => {
                log!("Warning: couldn't load icon for app bundle {}: {} (displaying placeholder instead)", app_path.display(), e);
                None
            }
        };

        apps.push(AppInfo {
            path: app_path,
            display_name,
            bundle_id,
            version,
            package_kind,
            icon,
            display_name_ns_string: None,
            icon_ui_image: None,
        });""",
    "enumerate",
)

t = t.replace('"touchHLE {}{}{}"', '"HyperHLE {}{}{}"', 1)

repl(
    """    let label_size = CGSize {
        width: 74.0,
        height: 13.0,
    };""",
    """    let label_size = CGSize {
        width: 78.0,
        height: 28.0,
    };""",
    "label size",
)

t = t.replace(
    """        y: 12.0,
    };

    let icon_tapped_sel""",
    """        y: 28.0,
    };

    let icon_tapped_sel""",
    1,
)

t = t.replace("let font_size: CGFloat = label_size.height - 2.0;", "let font_size: CGFloat = 10.0;", 1)
t = t.replace(
    "() = msg![env; label setFont:font];\n        let text_color: id = if have_wallpaper {",
    "() = msg![env; label setFont:font];\n        () = msg![env; label setNumberOfLines:2];\n        let text_color: id = if have_wallpaper {",
    1,
)

repl(
    """        let text = *app
            .display_name_ns_string
            .get_or_insert_with(|| ns_string::from_rust_string(env, app.display_name.clone()));
        () = msg![env; label setText:text];""",
    """        let label_text = {
            let mut line = app.display_name.clone();
            let mut meta: Vec<&str> = Vec::new();
            meta.push(app.package_kind);
            if let Some(ref v) = app.version {
                if !v.is_empty() {
                    let short = if v.len() > 12 { &v[..12] } else { v.as_str() };
                    meta.push(short);
                }
            }
            line.push('\\n');
            line.push_str(&meta.join(" · "));
            line
        };
        let text = *app.display_name_ns_string.get_or_insert_with(|| {
            ns_string::from_rust_string(env, label_text)
        });
        () = msg![env; label setText:text];""",
    "label text",
)

if "HyperHLE  ·" not in t:
    marker = "    // Version label\n"
    header = """    // HyperHLE header
    {
        let label_frame = CGRect {
            origin: CGPoint { x: 12.0, y: 4.0 },
            size: CGSize { width: app_frame.size.width - 24.0, height: 22.0 },
        };
        let label: id = msg_class![env; UILabel alloc];
        let label: id = msg![env; label initWithFrame:label_frame];
        let header_text = match &apps {
            Ok(list) => format!("HyperHLE  ·  {} game{}", list.len(), if list.len() == 1 { "" } else { "s" }),
            Err(_) => "HyperHLE".to_string(),
        };
        let text = ns_string::from_rust_string(env, header_text);
        () = msg![env; label setText:text];
        () = msg![env; label setTextAlignment:UITextAlignmentCenter];
        let font: id = msg_class![env; UIFont boldSystemFontOfSize:(16.0 as CGFloat)];
        () = msg![env; label setFont:font];
        let text_color: id = if have_wallpaper { msg_class![env; UIColor whiteColor] } else { msg_class![env; UIColor cyanColor] };
        () = msg![env; label setTextColor:text_color];
        let bg_color: id = msg_class![env; UIColor clearColor];
        () = msg![env; label setBackgroundColor:bg_color];
        () = msg![env; main_view addSubview:label];
    }

    // Version label
"""
    if marker not in t:
        raise SystemExit("Version label marker missing")
    t = t.replace(marker, header, 1)
    print("ok header")

p.write_text(t)
print("DONE", len(t), "package_kind" in t)
