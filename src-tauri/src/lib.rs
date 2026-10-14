mod commands;
mod cram;
mod written;
mod db;
mod embed;
mod export;
mod import;
mod plan;
mod quality;
mod queue;
mod render;
mod source;
mod srs;
mod sync;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // %APPDATA%\<식별자> (DECISIONS H1: 식별자가 바뀌면 이 경로가 바뀐다).
            // NEOANKI_DATA_DIR 로 바꿀 수 있다. 테스트가 실제 데이터를 건드리지 않게 하기 위한 것이다
            let data_dir = match std::env::var_os("NEOANKI_DATA_DIR") {
                Some(dir) => dir.into(),
                None => app.path().app_data_dir()?,
            };
            std::fs::create_dir_all(&data_dir)?;
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
            cram::cram_plan, cram::cram_cards, cram::cram_answer, cram::cram_undo,
            // I 원자료 (자료 · 강조 구간 · 카드 연결 · 자료 기반 중요도 제안)
            source::source_list, source::source_add, source::source_delete, source::source_bytes, source::source_save_pdf,
            source::source_segments, source::source_slide, source::source_link, source::source_unlink, source::source_for_card,
            source::source_analyze, source::source_unanalyzed, embed::model_status, embed::model_download, embed::model_cancel, embed::model_delete,
            quality::quality_check,
            quality::hide_quality,
            quality::split_suggest,
            quality::split_apply,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
