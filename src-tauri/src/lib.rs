mod ai;
mod commands;
mod cram;
mod written;
mod db;
mod embed;
mod exam;
mod export;
mod import;
mod notetype;
mod occlusion;
mod plan;
mod quality;
mod rag;
mod recall;
mod queue;
mod render;
mod source;
mod table;
mod srs;
mod sync;
mod tts;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // 첫 창은 여기서 만든다 (tauri.conf.json create: false). 만들기 전에 크기를 정해 커졌다 줄어드는 깜빡임이 없다.
            // 켤 때마다 주 모니터 작업 영역에 맞춘다. 모니터를 못 읽으면 설정 크기(1280×800) 그대로
            // ponytail: 크기 기억은 하지 않는다. 필요하면 닫을 때 크기를 저장해 여기서 읽는다
            if let Some(conf) = app.config().app.windows.first().cloned() {
                let mut win = tauri::WebviewWindowBuilder::from_config(app.handle(), &conf)?;
                let area = app.primary_monitor().ok().flatten().map(|m| m.work_area().size.to_logical::<f64>(m.scale_factor()));
                if let Some((w, h)) = area.and_then(|a| fit_window(a.width, a.height)) {
                    win = win.inner_size(w, h).center();
                }
                win.build()?;
            }
            // %APPDATA%\<식별자> (DECISIONS H1: 식별자가 바뀌면 이 경로가 바뀐다).
            // NEOANKI_DATA_DIR 로 바꿀 수 있다. 테스트가 실제 데이터를 건드리지 않게 하기 위한 것이다
            let data_dir = match std::env::var_os("NEOANKI_DATA_DIR") {
                Some(dir) => dir.into(),
                None => app.path().app_data_dir()?,
            };
            std::fs::create_dir_all(&data_dir)?;
            // 지난 실행이 가져오는 중에 끊겨 남긴 임시 컬렉션 (T7-05)
            import::clean_stale(&data_dir);
            // 열지 못해도 앱은 뜬다. 빈 메모리 DB 로 시작하고 화면이 S1-6 안내(다시 시도 / 백업에서 복구)를 띄운다.
            // 자동 복구는 하지 않는다. 원본 파일을 건드리기 전에 사용자가 고른다
            let (con, startup_error) = match db::open(&data_dir.join("neoanki.sqlite")) {
                Ok(con) => (con, None),
                Err(e) => (db::open_in_memory()?, Some(e.to_string())),
            };
            app.manage(commands::AppState {
                db: std::sync::Mutex::new(con),
                startup_error: std::sync::Mutex::new(startup_error),
                fsrs: fsrs::FSRS::default(),
                data_dir,
                undo: std::sync::Mutex::new(vec![]),
                import_cancel: std::sync::atomic::AtomicBool::new(false),
            });
            // K Google Drive 동기화: 상태 · 백그라운드 작업 (연결하지 않았으면 네트워크를 쓰지 않는다)
            sync::setup(app.handle());
            // G2 성격별로 묶기 · ChatGPT 요금제 (연결하지 않았으면 네트워크를 쓰지 않는다)
            app.manage(ai::AiState::new());
            Ok(())
        })
        // K8 창을 닫을 때 남은 변경을 최대 10초 올리고 닫는다
        .on_window_event(sync::on_window_event)
        .invoke_handler(tauri::generate_handler![
            commands::home,
            commands::create_deck,
            commands::rename_deck,
            commands::archive_deck,
            commands::delete_deck,
            commands::archived_decks,
            commands::set_deck_paused,
            commands::unarchive_deck,
            commands::deck_detail,
            commands::add_card,
            commands::list_cards,
            commands::set_cards_importance,
            commands::delete_cards,
            commands::move_cards,
            commands::bury_cards,
            commands::set_deck_weak,
            commands::dismiss_weak,
            commands::unbury_cards,
            commands::heatmap,
            commands::note_edit,
            commands::update_note,
            commands::save_deck_config,
            commands::preview_deck_config,
            commands::set_deck_importance,
            commands::confirm_importance,
            commands::importance_board,
            commands::queue_preview,
            commands::start_session,
            commands::card_view,
            commands::answer,
            commands::bury_card,
            commands::undo_last,
            commands::unbury_card,
            commands::finish_session,
            commands::resolve_buried,
            commands::save_media,
            commands::read_media,
            commands::app_info,
            commands::mvp_metrics,
            commands::create_backup,
            commands::retry_open,
            commands::restore_backup,
            commands::open_backup_dir,
            commands::set_cutoff_hour,
            commands::wipe_all,
            import::preview_json,
            import::import_json,
            import::inspect_anki,
            import::import_anki,
            import::cancel_import,
            import::save_text,
            import::read_json_file,
            import::read_table_file,
            export::export_apkg,
            export::cancel_export,
            export::export_preview,
            export::reveal_file,
            export::export_csv,
            // J 서술형 · G3 신호 · H3 압축 회독 (학습 화면)
            written::written_card, written::save_written_answer, written::compare_written, written::written_note, written::save_written_note,
            written::suggest_keywords, written::add_written_card, written::split_signal,
            cram::cram_plan, cram::cram_cards, cram::cram_answer, cram::cram_undo, cram::mix_cards, cram::mix_start, cram::mix_restart, cram::mix_summary,
            // M 그림 가리기
            occlusion::occlusion_note, occlusion::save_occlusion,
            // Q 목록 회상 · 선택형
            recall::add_list_card, recall::add_choice_card, recall::recall_suggest, recall::save_recall_marks,
            recall::update_list_note, recall::update_choice_note,
            // 8.1 노트 유형 · 카드 템플릿 (1단계 엔진, 화면은 2단계)
            notetype::note_types, notetype::note_type_detail, notetype::deck_note_type, notetype::clone_note_type, notetype::rename_note_type,
            notetype::delete_note_type, notetype::add_note_field, notetype::rename_note_field, notetype::reorder_note_fields,
            notetype::delete_note_field, notetype::add_card_template, notetype::delete_card_template, notetype::save_card_template,
            notetype::save_note_type_css, notetype::note_type_empty_fronts, notetype::preview_template, notetype::add_typed_note,
            notetype::template_layout, notetype::build_template_layout, notetype::suggest_card_kind,
            // 8.4 기출 이력
            exam::mark_exam, exam::set_exam, exam::exam_list, exam::exam_summary,
            // I 원자료 (자료 · 강조 구간 · 카드 연결 · 자료 기반 중요도 제안)
            source::source_list, source::source_add, source::source_delete, source::source_bytes, source::source_save_pdf,
            source::source_segments, source::source_slide, source::source_link, source::source_unlink, source::source_for_card,
            source::source_analyze, source::source_unanalyzed, source::source_segment, source::source_region_link, source::source_region_cards,
            source::source_slide_objects, source::source_slide_image, source::source_region_deck, table::source_table_cards, rag::rag_preview, rag::rag_draft, rag::rag_draft_apply, rag::rag_grade_scope, rag::rag_grade, rag::rag_linked, embed::model_status, embed::model_download, embed::model_cancel, embed::model_delete,
            quality::quality_check,
            quality::hide_quality,
            quality::split_suggest,
            quality::split_apply,
            ai::ai_status, ai::ai_connect, ai::ai_connect_cancel, ai::ai_disconnect, ai::ai_open_usage, ai::ai_regroup, ai::ai_cancel,
            quality::undo_change,
            quality::recent_changes,
            plan::study_plan,
            plan::set_study_time,
            plan::pull_plan,
            plan::set_recompute,
            plan::plan_conflict,
            plan::resolve_plan_conflict,
            plan::plan_adjust,
            // K Google Drive 보관 · 동기화
            sync::sync_status, sync::sync_pause, sync::sync_now, sync::sync_close_now, sync::sync_connect, sync::sync_connect_cancel,
            sync::sync_probe, sync::sync_join, sync::sync_disconnect, sync::sync_settings, sync::sync_set_lock, sync::sync_unlock,
            sync::sync_backups, sync::sync_restore_preview, sync::sync_restore, sync::sync_backup_full, sync::sync_delete_backup,
            sync::sync_prune_backups, sync::sync_open_folder, sync::sync_conflicts, sync::sync_conflict_use, sync::sync_conflicts_seen,
            // AU20 읽어주기
            tts::tts_langs, tts::tts_preload, tts::tts_speak, tts::tts_license,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// 첫 창 크기 (논리 px). 설정 크기 1280×800 을 작업 영역의 90% 와 각각 비교해 작은 쪽, 최소 1180×640.
/// 그대로면 None (위치도 건드리지 않는다)
fn fit_window(area_w: f64, area_h: f64) -> Option<(f64, f64)> {
    let w = (area_w * 0.9).clamp(1180.0, 1280.0);
    let h = (area_h * 0.9).clamp(640.0, 800.0);
    (w < 1280.0 || h < 800.0).then_some((w, h))
}

#[cfg(test)]
mod tests {
    #[test]
    fn fit_window() {
        // 1080p 150%: 작업 영역 약 1280×672 → 90% 는 1152×604.8 이라 최소 크기로
        assert_eq!(super::fit_window(1280.0, 672.0), Some((1180.0, 640.0)));
        // 1366×768 100% (작업 영역 1366×728): 1229.4×655.2
        let (w, h) = super::fit_window(1366.0, 728.0).unwrap();
        assert!((w - 1229.4).abs() < 1e-9 && (h - 655.2).abs() < 1e-9);
        // 넉넉한 화면은 그대로
        assert_eq!(super::fit_window(2560.0, 1392.0), None);
        assert_eq!(super::fit_window(1920.0, 1040.0), None);
    }
}
