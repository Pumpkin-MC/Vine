use super::PluginInitError;

#[derive(Debug, Default, Eq, PartialEq)]
pub struct ComponentNames {
    /// Exported `vine:plugin/metadata@...` interface version, such as `"0.1.0"`.
    pub metadata_version: Option<String>,
    /// First imported WASI 0.3 interface, if any.
    pub wasi_p3_import: Option<String>,
    /// Guest binding language inferred from the WebAssembly `producers` custom section.
    pub binding_language: Option<String>,
}

/// Scans top-level imports/exports and `producers` custom sections of a WebAssembly component.
pub fn scan_component_names(wasm_bytes: &[u8]) -> Result<ComponentNames, PluginInitError> {
    let malformed = |error: wasmparser::BinaryReaderError| {
        PluginInitError::ComponentCompilationFailed(wasmtime::Error::msg(error.to_string()))
    };
    let mut names = ComponentNames::default();
    let mut language_detector = LanguageDetector::default();
    let mut depth = 0usize;

    for payload in wasmparser::Parser::new(0).parse_all(wasm_bytes) {
        match payload.map_err(malformed)? {
            wasmparser::Payload::ModuleSection { .. }
            | wasmparser::Payload::ComponentSection { .. } => depth += 1,
            wasmparser::Payload::End(_) => depth = depth.saturating_sub(1),
            wasmparser::Payload::ComponentImportSection(imports) if depth == 0 => {
                for import in imports {
                    let name = import.map_err(malformed)?.name.full_name();
                    if names.wasi_p3_import.is_none() && is_wasi_p3_import(&name) {
                        names.wasi_p3_import = Some(name.into_owned());
                    }
                }
            }
            wasmparser::Payload::ComponentExportSection(exports) if depth == 0 => {
                for export in exports {
                    let name = export.map_err(malformed)?.name.full_name();
                    if let Some(version) = name.strip_prefix("vine:plugin/metadata@") {
                        names.metadata_version = Some(version.to_owned());
                    }
                }
            }
            wasmparser::Payload::CustomSection(section) => {
                if let wasmparser::KnownCustom::Producers(producers) = section.as_known() {
                    language_detector.observe_section(producers);
                }
            }
            _ => {}
        }
    }

    names.binding_language = language_detector.finish();
    Ok(names)
}

#[derive(Default)]
struct LanguageDetector {
    best: Option<(u8, String)>,
    saw_cpp: bool,
}

impl LanguageDetector {
    fn observe_section(&mut self, producers: wasmparser::ProducersSectionReader<'_>) {
        let mut section_has_rust = false;
        let mut section_candidates: Vec<(u8, String)> = Vec::new();

        for field in producers.into_iter().flatten() {
            for value in field.values.into_iter().flatten() {
                match field.name {
                    "processed-by" | "sdk" => {
                        if let Some((priority, lang)) = Self::classify_tool(value.name) {
                            if lang == "rust" {
                                section_has_rust = true;
                            } else if lang == "cpp" {
                                self.saw_cpp = true;
                            }
                            section_candidates.push((priority, lang.to_owned()));
                        }
                    }
                    "language" => {
                        if let Some((priority, lang)) = Self::classify_language(value.name) {
                            if lang == "rust" {
                                section_has_rust = true;
                            } else if lang == "cpp" {
                                self.saw_cpp = true;
                            }
                            section_candidates.push((priority, lang));
                        }
                    }
                    _ => {}
                }
            }
        }

        for (priority, lang) in section_candidates {
            if section_has_rust && lang == "c" && priority < 3 {
                continue;
            }
            if self.best.as_ref().is_none_or(|(p, _)| priority > *p) {
                self.best = Some((priority, lang));
            }
        }
    }

    fn classify_tool(tool: &str) -> Option<(u8, &'static str)> {
        let lower = tool.trim().to_ascii_lowercase();
        if lower.contains("componentize-py") {
            Some((4, "python"))
        } else if lower.contains("componentize-js")
            || lower.contains("componentizejs")
            || lower == "jco"
            || lower.contains("starlingmonkey")
        {
            Some((4, "javascript"))
        } else if lower.contains("componentize-dotnet") || lower.contains("wit-bindgen-csharp") {
            Some((4, "csharp"))
        } else if lower.contains("wit-bindgen-go") || lower == "tinygo" {
            Some((4, "go"))
        } else if lower.contains("wit-bindgen-moonbit") {
            Some((4, "moonbit"))
        } else if lower.contains("wit-bindgen-teavm-java") {
            Some((4, "java"))
        } else if lower.contains("wit-bindgen-c") {
            Some((3, "c"))
        } else if lower.contains("wit-bindgen-rust") || lower.contains("cargo-component") {
            Some((3, "rust"))
        } else if lower == "clang++" {
            Some((2, "cpp"))
        } else if lower == "clang" {
            Some((1, "c"))
        } else if lower == "rustc" {
            Some((1, "rust"))
        } else {
            None
        }
    }

    fn classify_language(language: &str) -> Option<(u8, String)> {
        let lower = language.trim().to_ascii_lowercase();
        match lower.as_str() {
            "" | "wat" => None,
            "rust" => Some((1, "rust".to_owned())),
            "c_plus_plus" | "c++" | "cpp" => Some((2, "cpp".to_owned())),
            "c" => Some((2, "c".to_owned())),
            "c#" | "csharp" => Some((4, "csharp".to_owned())),
            "javascript" | "js" => Some((4, "javascript".to_owned())),
            "typescript" | "ts" => Some((4, "typescript".to_owned())),
            "python" | "py" => Some((4, "python".to_owned())),
            "go" | "golang" => Some((4, "go".to_owned())),
            other => Some((4, other.to_owned())),
        }
    }

    fn finish(self) -> Option<String> {
        self.best.map(|(_, lang)| {
            if lang == "c" && self.saw_cpp {
                "cpp".to_owned()
            } else {
                lang
            }
        })
    }
}

fn is_wasi_p3_import(name: &str) -> bool {
    name.starts_with("wasi:")
        && name
            .rsplit_once('@')
            .is_some_and(|(_, version)| version.starts_with("0.3."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_encoder::{
        ComponentBuilder, ComponentExportKind, ComponentTypeRef, InstanceType, Module,
        ProducersField, ProducersSection,
    };

    fn module_with_producers(languages: &[&str], tools: &[&str]) -> Module {
        let mut producers = ProducersSection::new();
        if !languages.is_empty() {
            let mut field = ProducersField::new();
            for &lang in languages {
                field.value(lang, "1.0");
            }
            producers.field("language", &field);
        }
        if !tools.is_empty() {
            let mut field = ProducersField::new();
            for &tool in tools {
                field.value(tool, "1.0");
            }
            producers.field("processed-by", &field);
        }
        let mut module = Module::new();
        module.section(&producers);
        module
    }

    #[test]
    fn detects_metadata_version_and_wasi_p3_imports() {
        let mut component = ComponentBuilder::default();
        let ty = component.type_instance(None, &InstanceType::new());
        let inst = component.import("wasi:random/random@0.3.0", ComponentTypeRef::Instance(ty));
        component.export(
            "vine:plugin/metadata@0.1.0",
            ComponentExportKind::Instance,
            inst,
            None,
        );
        let bytes = component.finish();
        let names = scan_component_names(&bytes).unwrap();
        assert_eq!(names.metadata_version.as_deref(), Some("0.1.0"));
        assert_eq!(
            names.wasi_p3_import.as_deref(),
            Some("wasi:random/random@0.3.0")
        );
    }

    #[test]
    fn detects_binding_languages_from_producers_sections() {
        let mut rust_comp = ComponentBuilder::default();
        rust_comp.core_module(
            None,
            &module_with_producers(&["C", "Rust"], &["clang", "rustc", "wit-bindgen-rust"]),
        );
        assert_eq!(
            scan_component_names(&rust_comp.finish())
                .unwrap()
                .binding_language
                .as_deref(),
            Some("rust")
        );

        let mut go_comp = ComponentBuilder::default();
        go_comp.core_module(
            None,
            &module_with_producers(&["Go"], &["tinygo", "wit-bindgen-go"]),
        );
        go_comp.core_module(None, &module_with_producers(&["C", "Rust"], &["rustc"]));
        assert_eq!(
            scan_component_names(&go_comp.finish())
                .unwrap()
                .binding_language
                .as_deref(),
            Some("go")
        );
    }

    #[test]
    fn rejects_invalid_wasm_bytes() {
        assert!(scan_component_names(b"not-a-wasm-binary").is_err());
    }
}
