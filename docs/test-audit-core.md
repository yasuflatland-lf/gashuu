# Core test audit (#758)

At `38332cb`, the 16 in-scope test files contain 505 test functions, including feature-gated tests and properties. Grouping the current tests by behavior and including groups removed by earlier fixes gives **272 groups**. Earlier work accounts for **44**. The remaining **228** have one verdict each: **1 delete, 19 strengthen, 9 property, 199 keep**. The original list of about 173 groups was unavailable, so these counts describe the reconstructed inventory.

I read the tests against the implementation and checked the merged property diffs (`ceca085`, `bd110a2`, `5befaf3`) and named fixes. For the mutation checks I changed one production expression at a time, ran the selected group with pinned Cargo nextest, then restored the file with Git and checked it against HEAD. Of 34 groups checked with mutations, 20 survived (all selected tests passed) and 14 caught the change (at least one failed). The other 194 groups are keep decisions from reading. Each keep row with a mutation names the change. Estimates are rough test lines touched in a follow-up PR (zero for keep). File and line references are relative to the owning crate’s `src/` unless otherwise qualified.

The five protected audit IDs **C1-27, C1-48, U1-45, U2-3 and U2-26** retain their **keep** disposition. The checkout contains no ID-to-test crosswalk. The counts above include only identifiable behaviors. The original TEST IDs and the property-candidate IDs use overlapping numbers.

## Excluded groups

Numbers within a row identify separate behavior groups. Names identify the current witness or the removed behavior. An older fix later subsumed by a property is counted under its final owner. Test-module moves (#745–#751) changed locations only.

| Covering issue / merged PR | Groups | Behavior already addressed |
| --- | ---: | --- |
| #716 / #765 | 1 | (1) `mixed_case_page_names_list_in_numeric_order` |
| #722 / #763 | 5 | (1) `normalize_merges_exact_duplicate_ids`, `normalize_dedup_keeps_set_resume_page_on_the_only_copy` (2) `non_default_settings_differs_from_default_in_every_field` (3) `migrate_noop_for_current_version` (4) `from_json_huge_version_is_treated_as_unknown_and_migrates` (5) duplicate Book title derivation |
| #725 / #771 | 2 | (1) `open_book_rejects_zero_page_folder_as_empty_book`, `open_book_counts_image_pages`, `open_book_read_at_empty_zip_is_empty_book` (2) `detect_prefers_extension_over_magic_on_both_routes` |
| #728 / #769 | 4 | (1) `cache_capacity_follows_the_config_floor` (2) `prefetch_keeps_an_already_cached_next_page` (3) `promote_window_orders_center_then_nearest_neighbours` (4) `decoded_lru_promote_reports_presence` |
| #729 / #776 | 1 | (1) `cache_config_capacity_is_at_least_the_window_floor` |
| #733 / #780 | 2 | (1) `view_defaults_copies_the_four_global_modes`, `set_view_defaults_is_the_inverse_of_view_defaults` (2) `set_prefetch_radius_raises_capacity_to_the_window_floor`, `set_cache_capacity_clamps_and_persists` |
| #740 / #791 | 1 | (1) `scrub_rtl_is_the_mirror_of_ltr_at_the_ends` |
| #742 / #785 | 1 | (1) `prop_fits_in_quota_is_monotone_in_size` |
| #744 / #795 | 6 | (1) `format_disabled_carries_archive_kind` (2) `archive_kind_wire_round_trip` (3) `open_read_at_rejects_rar_by_signature` (4) `shorter_replacement_reports_source_changed` (5) `maps_core_errors` (6) `classifies_dom_exceptions_by_name` |
| #752 / #804 | 9 | (1) `prop_folder_lists_images_within_size_limit` (2) `prop_read_at_cursor_matches_std_cursor` (3) `prop_prefetch_indices_match_neighbourhood` (4) `prop_decoded_lru_matches_model` (5) `decode_thumbnail_wide_image_fits_max_side`, `decode_thumbnail_tall_image_fits_max_side`, `decode_thumbnail_square_image_fits_max_side`, `decode_thumbnail_jpeg_wide_image_fits_max_side`, `decode_thumbnail_avif_fits_max_side` (6) complete folder default (7) permissive RAR open alias (8) duplicate default-ceiling RAR read (9) duplicate small-PNG decode |
| #753 / #806 | 10 | (1) `remove_then_restore_reproduces_library_json` (2) `restore_does_not_overwrite_newer_progress` (3) `progress_setters_follow_the_model` (4) `normalized_settings_round_trip_through_json` (5) `thumbnail_keys_follow_the_legacy_recipe` (6) `source_mtime_secs_reads_real_file_mtime` (7) `library_json_round_trips_byte_identically` (8) `library_store_round_trips_byte_identically` (9) non-UTF-8 BookId duplicate (10) orphan-on-load duplicate |
| #754 / #807 | 2 | (1) `cover_prune_removes_minimal_oldest_first_prefix` (2) inert staged page-purge/prune assertions |

Of the 44 exclusions, 12 are property groups from the three requested issues (4 in #752, 7 in #753 and 1 in #754). The other 32 are named fixes, earlier coverage or duplicate removals. For #752 the four in-scope properties cover prefetch neighborhoods, the decoded LRU model, folder membership and the positional cursor. For #753 the two restore properties also cover the stale-copy and removed-book fixes from #722. The thumbnail-key recipe moved through #734 before #753 added the legacy oracle. For #754 only the prune planner and the inert staged-store assertions are excluded here. The quota property predates that change (#742). The remaining staged-store operations have their own rows.

## [spread.rs](../crates/gashuu-core/src/spread.rs)

Tests in [spread/tests.rs](../crates/gashuu-core/src/spread/tests.rs).

| Group | Tests | Verdict | Reason / mutation not caught | Est. diff |
| --- | --- | --- | --- | ---: |
| Single-page spread | `single_spread_at_trailing_always_none`, `single_spread_at_clamps_oob_leading` | keep | A clamped leading page has no partner under either cover policy. | 0 |
| Single-page steps | `single_next_clamps_at_end`, `single_prev_clamps_at_start` | keep | Forward and backward movement stop at opposite ends of the book. | 0 |
| Single-page normalization | `single_normalize_is_identity_with_clamp` | keep | The first, last and out-of-range raw indices have explicit destinations. | 0 |
| Empty and one-page navigation | `single_does_not_panic_on_tiny_totals`, `double_paired_does_not_panic_on_tiny_totals`, `double_standalone_does_not_panic_on_tiny_totals` | keep | Zero is a usable sentinel even when there is no valid pair. | 0 |
| Paired spreads | `double_paired_spread_at_full_pairs`, `double_paired_spread_at_last_partial_when_odd_total`, `double_paired_spread_at_total2` | keep | Protect the cover pair and the unpaired final page of an odd-length book. | 0 |
| Paired steps | `double_paired_next_advances_by_two_and_clamps`, `double_paired_prev_steps_back_by_two_and_clamps`, `double_paired_next_defensive_odd_leading`, `double_paired_prev_defensive_odd_leading`, `double_paired_next_prev_always_even_for_any_leading` | property | From any input, a turn selects the adjacent pair in reading order or stays at the boundary, including when the input names a trailing page. Returning zero for the forward paired step at spread.rs:133 was caught. | 40-75 |
| Paired normalization | `double_paired_normalize_rounds_down_to_even` | keep | A trailing page jumps back onto its even leading page. | 0 |
| Standalone spreads | `double_standalone_spread_at_cover_then_pairs`, `double_standalone_spread_at_small_totals` | keep | The cover occupies its own spread even when only two pages exist. | 0 |
| Final standalone start | `double_standalone_last_start_values` | keep | Odd and even totals place the final spread at different indices. | 0 |
| Standalone turns and normalization | `double_standalone_next`, `double_standalone_prev`, `double_standalone_normalize`, `double_standalone_next_defensive_even_leading`, `double_standalone_prev_defensive_even_leading` | keep | Explicit destinations pin the transition across the solo cover and defensive even inputs. | 0 |
| Toggle containment | `toggle_boundary_single_index_5`, `toggle_boundary_visible_page_stays_contained` | property | For every nonempty book and cover/layout combination, the spread selected after normalization contains the requested page clamped into the book. Returning zero for paired normalization at spread.rs:213 was caught. | 20-40 |
| Reversible turns | `next_prev_round_trip_double_paired`, `next_prev_round_trip_double_standalone` | strengthen | Survived `spread.rs:139` `first_pair_start(total) -> 0`. Start at the standalone cover as well as inside the book and assert both the destination and return. | 4-18 |
| Context delegation | `spread_context_delegates_to_free_fns`, `spread_context_delegates_standalone`, `spread_context_delegates_single_layout` | strengthen | Survived `spread.rs:108` `trailing < total -> false` in all three tests because both sides call the same helper. Compare visible pages and turn destinations to literal spreads. | 8-30 |
| Context value semantics | `spread_context_is_copy_and_eq` | strengthen | Survived `spread.rs:247` `total, -> total: 0,`. Preserve the Copy constraint and compare contexts built with different totals and their final spreads. | 4-18 |
| Valid navigation index | `index_is_always_a_valid_leading` | property | After any sequence of turns, jumps and mode changes the index is the first page of an actual spread, or zero for an empty book. Returning zero from context normalization at spread.rs:258 was caught. | 20-40 |
| Mode change then jump | `mode_change_then_jump_lands_on_the_containing_spread` | keep | A jump after a mode switch must still contain the requested page. | 0 |
| Viewport transition | `set_viewport_size_returns_true_only_when_the_effective_layout_flips`, `navigation_viewport_flip_renormalizes_and_fixed_modes_ignore_aspect` | keep | Only a change in effective layout signals a layout flip and moves a now-invalid leading page. | 0 |
| New total | `set_total_resets_the_index` | keep | Replacing the book starts navigation at zero. | 0 |
| Empty turns | `next_prev_are_no_ops_at_zero_total` | keep | Both commands report no movement with no pages. | 0 |
| Resume index | `navigation_resume_index_to_persist_cases` | keep | Completing a book on a trailing page persists the final page. | 0 |
| Navigation turns | `navigation_single_next_prev_and_clamping`, `navigation_double_standalone_advances_by_spread`, `navigation_double_paired_advances_by_spread` | keep | Public navigation returns the expected sequence and movement flags for each layout. | 0 |
| Jump | `navigation_jump_to_cases` | keep | Literal destinations and changed flags distinguish jumps within and between spreads. | 0 |
| Preview | `navigation_preview_spread_cases`, `navigation_preview_is_double_cases` | keep | A preview has the expected leading and trailing pages without moving the reader. | 0 |
| Mode setters | `navigation_mode_setters_are_idempotent_and_renormalize` | keep | Changing mode repairs the position and repeating the change reports a no-op. | 0 |
| Auto layout | `navigation_auto_resolves_from_viewport`, `navigation_auto_portrait_with_paired_cover_advances_by_spread`, `navigation_set_auto_uses_current_viewport` | keep | A stored viewport governs pairing both on a turn and when entering Auto. | 0 |
| Empty or one-page scrub | `scrub_fraction_zero_count_is_zero_guard`, `scrub_fraction_non_finite_single_page_is_zero`, `scrub_fraction_single_page_is_always_zero` | keep | Both directions map to zero, including non-finite inputs on a one-page book. | 0 |
| Scrub track | `scrub_fraction_ltr_maps_ends_and_midpoint`, `scrub_fraction_rtl_inverts_fraction`, `scrub_fraction_clamps_out_of_range_input` | keep | Concrete page numbers distinguish direction, endpoint clamping and midpoint selection. | 0 |
| Half-step rounding | `scrub_fraction_rounds_half_up_at_exact_half_step`, `scrub_fraction_odd_span_rtl_half_step_rounds_half_up` | keep | Exact ties and the point just below a tie distinguish rounding policies, including mirrored odd spans. | 0 |
| Non-finite scrub | `scrub_fraction_is_total_function_no_nan_panic` | keep | A multi-page book distinguishes the two direction fallbacks. A constant zero result at spread.rs:482 was caught. | 0 |
| Scrub commit | `scrub_commit_path_jumps_via_jump_to` | keep | Passing the computed endpoint to jump moves the real navigation index in both directions. | 0 |

## [library.rs](../crates/gashuu-core/src/library.rs)

Tests in [library/tests.rs](../crates/gashuu-core/src/library/tests.rs).

| Group | Tests | Verdict | Reason / mutation not caught | Est. diff |
| --- | --- | --- | --- | ---: |
| Display title | `display_title_uses_dir_name_for_real_folder`, `display_title_uses_file_stem_for_archive_path`, `display_title_keeps_leading_dot_name_as_stem`, `display_title_falls_back_to_lossy_path_when_no_file_component`, `display_title_falls_back_to_lossy_path_for_root`, `display_title_is_pure_given_is_dir` | keep | Dotted directories, archive stems, dotfiles and component-less paths distinguish the naming rules. | 0 |
| Book lookup | `book_lookup_by_id` | keep | Checks both a matching identity and a missing one. | 0 |
| Removal report order | `remove_many_report_order_matches_path_order` | keep | Path-component order differs from raw string order for the chosen IDs. | 0 |
| Reopen title | `register_opened_uses_caller_title_for_a_new_book_and_keeps_stored_title_on_reopen` | keep | Reopening with a new caller title preserves the shelved title. | 0 |
| Canonical identity | `canonical_identity_returns_the_canonical_path_for_a_real_file`, `canonical_identity_falls_back_for_a_missing_path`, `canonical_identity_falls_back_when_the_canonical_form_is_not_utf8`, `a_library_built_through_canonical_identity_always_serializes` | keep | Existing files resolve, missing paths retain their spelling, and a UTF-8 link to a non-UTF-8 target stays serializable on supported Unix filesystems. | 0 |
| Fresh shelf | `new_library_is_empty`, `last_opened_is_none_for_fresh_library` | keep | A new library has neither books nor a last-opened ID. | 0 |
| Clear | `clear_empty_library_returns_false`, `clear_non_empty_library_removes_books_and_last_opened` | keep | The populated case checks the persisted bookmark and books, and the empty case checks the change flag. | 0 |
| Shelf sort | `add_orders_books_by_natural_title`, `add_tie_breaks_same_title_by_canonical_path` | keep | Numeric titles and equal-title path ties have explicit expected order. | 0 |
| Add result | `add_dedups_by_path_and_returns_false`, `add_returns_stored_canonical_path_then_none_on_duplicate` | keep | Only the initial insert returns its stored ID and grows the shelf. | 0 |
| Batch equivalence | `add_many_matches_repeated_add` | property | For any starting shelf and input batch, bulk addition yields the same ordered shelf and sequence of accepted IDs as individual additions. Omitting the batch sort at library.rs:302 was caught. | 20-40 |
| Batch order and empty input | `add_many_sorts_once_into_natural_order`, `add_many_empty_input_is_a_no_op` | keep | Separates insertion order in the report from display order and checks byte stability of a no-op. | 0 |
| Caller identity | `add_stores_caller_supplied_canonical_identity`, `add_stores_the_given_identity_verbatim` | keep | Supplying the identity once prevents reinterpreting a path during insertion. | 0 |
| Remove one | `remove_existing_returns_true_and_drops_book`, `remove_absent_returns_false` | keep | Return value and membership distinguish an actual removal from a miss. | 0 |
| Remove many | `remove_many_removes_multiple_and_reports_them`, `remove_many_preserves_natural_order_of_survivors`, `remove_many_splits_not_found`, `remove_many_empty_input_yields_empty_report`, `remove_many_counts_duplicate_input_once` | keep | Checks survivor order, missing IDs and duplicate requests against the actual removal report. | 0 |
| Restore stale bookmark | `restore_drops_a_stale_bookmark_not_on_the_shelf` | keep | Restoring cannot leave a bookmark pointing off the shelf. | 0 |
| Fresh progress | `last_page_is_zero_for_unknown_path`, `page_count_opt_is_none_for_fresh_book`, `progress_is_at_start_for_freshly_added_book` | keep | Unknown IDs and freshly added books have defined resume and count fallbacks. | 0 |
| Shrinking book | `set_page_count_repairs_a_stale_resume_page`, `stale_pair_in_json_is_clamped_on_load` | keep | Both an updated count and loading a stale persisted pair bring resume back into range. | 0 |
| Progress schema | `book_json_shape_is_byte_identical`, `book_without_page_count_field_deserializes_to_unknown` | keep | The flat book JSON and missing count are compatibility requirements. | 0 |
| Book progress | `progress_reflects_last_page_and_page_count` | keep | The value exposed by a book reflects its recorded page and total. | 0 |
| Open count update | `register_opened_adds_fresh_book_and_back_fills_count`, `register_opened_is_idempotent_and_reports_no_change_when_count_unchanged`, `register_opened_skips_back_fill_when_count_unknown`, `register_opened_reports_count_change_on_new_nonzero_count` | keep | First open, unknown count and a measured count change drive distinct persistence decisions. | 0 |
| Open resume | `register_opened_resumes_at_recorded_last_page`, `register_opened_twice_preserves_recorded_resume_page` | keep | Opening repeatedly preserves a nonzero recorded position. | 0 |
| Open identity | `register_opened_with_caller_supplied_identity_keeps_invariant` | keep | After opening via a canonicalized detour, the last-opened ID refers to the shelved book. | 0 |
| Override access | `new_book_has_empty_overrides`, `set_overrides_updates_and_reports_change`, `set_overrides_clear_to_none_reports_change_and_empties`, `set_overrides_false_when_path_absent`, `overrides_for_absent_path_is_empty` | keep | Assignment, clearing and an absent ID have distinct values and change flags. | 0 |
| Last-opened assignment | `register_opened_sets_last_opened_on_fresh_book`, `register_opened_updates_last_opened_on_reopen`, `register_opened_same_book_twice_preserves_last_opened` | keep | Opening a different or current book updates the bookmark deterministically. | 0 |
| Removal and bookmark | `remove_last_opened_book_clears_last_opened`, `remove_different_book_preserves_last_opened`, `remove_many_containing_last_opened_clears_it`, `remove_many_not_containing_last_opened_preserves_it` | keep | Single and batch removals clear the bookmark only when its target disappears. | 0 |
| Bookmark lookup | `bookmark_is_some_when_last_opened_is_shelved`, `bookmark_is_none_when_last_opened_is_none`, `bookmark_is_none_for_orphan_last_opened` | keep | Resolves an on-shelf ID and handles absent and orphan IDs. | 0 |
| Normalize bookmark | `normalize_clears_orphan_last_opened`, `normalize_keeps_valid_last_opened` | keep | Loading preserves a valid target and clears an orphan. | 0 |
| Merge progress | `recanonicalize_merges_two_spellings_of_same_file`, `merge_group_clamps_the_survivor_position` | keep | Two spellings become one book and the merged position fits the surviving count. | 0 |
| Repair stored path | `recanonicalize_upgrades_single_noncanonical_path`, `recanonicalize_preserves_missing_path_unchanged` | keep | A resolvable spelling upgrades and a missing file stays on the shelf. | 0 |
| Merge bookmark | `recanonicalize_repoints_last_opened_to_survivor` | keep | The stored last-opened ID follows the surviving book. | 0 |
| Merge order | `recanonicalize_keeps_books_in_natural_order` | keep | Recanonicalization preserves natural order in the resulting shelf. | 0 |

## [settings.rs](../crates/gashuu-core/src/settings.rs)

Tests in [settings/tests.rs](../crates/gashuu-core/src/settings/tests.rs).

| Group | Tests | Verdict | Reason / mutation not caught | Est. diff |
| --- | --- | --- | --- | ---: |
| Fresh defaults | `default_settings_have_expected_values`, `fit_mode_defaults_to_whole`, `language_defaults_to_en`, `window_defaults_to_none` | keep | First-run modes, language, shortcuts and absence of window geometry are user-visible choices. | 0 |
| Invalid document | `from_json_future_version_errors`, `from_json_non_object_root_errors` | keep | Future versions and non-object roots take different typed failure paths. | 0 |
| Serialized version | `to_json_always_stamps_the_current_schema_version` | strengthen | Survived `settings.rs:148` `to_string_pretty(self) -> to_string_pretty(&Settings::default())`. Assert the normalized version alongside a changed field in the same emitted document. | 4-18 |
| Version normalization | `normalize_is_idempotent_for_the_version_field` | keep | Repeating normalization must preserve the first result even for a stale input version. | 0 |
| v0 migration | `migrate_v0_fills_preload_and_bumps_version`, `migrate_preserves_existing_preload_pages` | keep | Supplies a missing preload value while preserving an explicit old value. | 0 |
| Missing preferences | `from_json_defaults_missing_fields`, `from_json_empty_document_uses_serde_defaults`, `from_json_missing_fit_mode_defaults_to_whole`, `from_json_missing_language_defaults_to_en`, `from_json_missing_window_defaults_to_none`, `allow_rar_archives_missing_field_loads_as_true`, `missing_auto_update_check_loads_as_true` | keep | Old or partial documents receive the intended defaults, including RAR permission and update checks. | 0 |
| Unknown enum values | `from_json_unknown_fit_mode_value_errors`, `from_json_unknown_language_value_errors` | keep | Invalid persisted fit modes and language tags surface a load error. | 0 |
| Recent history | `push_recent_disabled_is_noop`, `push_recent_dedups_moves_to_front_and_caps`, `push_recent_promotes_existing_middle_entry` | keep | Disabled tracking, promotion, deduplication and capacity affect the observable list order. | 0 |
| Default JSON snapshot | `default_settings_json_snapshot` | keep | The persisted names, omissions and default values are an independent wire-format fixture. | 0 |
| History on load | `from_json_caps_recent_sources_on_load` | keep | Oversize history is trimmed when loading old settings. | 0 |
| Language encoding | `language_serializes_as_ietf_tag` | keep | Locale tags in JSON use the expected spelling. | 0 |
| Legacy cache fields | `cache_config_reflects_fields`, `from_json_old_flat_keys_load_into_cache_config` | keep | Old flat keys still configure the effective cache capacity and radius. | 0 |
| Cache bounds on load | `from_json_clamps_cache_size_above_max`, `from_json_clamps_preload_pages_above_max`, `from_json_preserves_preload_pages_zero`, `normalize_clamps_out_of_range_fields` | keep | Malformed high values are bounded and explicit zero radius still disables prefetch. | 0 |
| Window omission | `window_omitted_from_default_json` | keep | A fresh settings save omits the window key. | 0 |
| Window normalization | `normalize_floors_window_size`, `normalize_discards_inflated_window_geometry`, `normalize_keeps_a_sane_window_geometry` | keep | Undersize, implausibly large and sane rectangles exercise different restoration decisions. | 0 |
| Update metadata | `none_skipped_and_last_check_are_omitted_from_json` | keep | Absent skipped-version and last-check values leave no JSON keys. | 0 |

## [thumbnail_cache.rs](../crates/gashuu-core/src/thumbnail_cache.rs)

Tests in [thumbnail_cache/tests.rs](../crates/gashuu-core/src/thumbnail_cache/tests.rs).

| Group | Tests | Verdict | Reason / mutation not caught | Est. diff |
| --- | --- | --- | --- | ---: |
| On-disk key compatibility | `fs_store_writes_the_legacy_file_name` | keep | A real store write must produce the legacy file name. | 0 |
| Store fingerprint | `fs_store_fingerprint_uses_current_mtime` | keep | Changing the source timestamp changes the fingerprint used by the store. | 0 |
| Null store | `null_store_is_inert` | keep | The no-storage implementation must remain inert across reads, writes and maintenance. | 0 |
| Missing source | `source_mtime_secs_returns_zero_for_missing_path` | keep | Missing metadata uses the defined zero timestamp. | 0 |
| Cache write location | `put_creates_directory_if_absent`, `put_writes_key_dot_qoi_inside_dir` | keep | Writing into a missing directory produces the keyed QOI file inside that directory. | 0 |
| Cache misses | `get_missing_key_returns_none`, `get_corrupt_file_returns_none`, `get_ignores_legacy_png_as_a_clean_miss` | keep | Missing, corrupt and valid legacy PNG entries all yield a recoverable miss. | 0 |
| QOI round trip | `put_get_roundtrip_preserves_dims_and_bytes`, `put_get_roundtrip_preserves_multicolor_pixels` | keep | Reloaded dimensions and pixels, including a multicolor pattern, catch encoding and channel-order regressions. | 0 |
| Overwrite | `put_twice_same_key_returns_latest_image` | keep | A second write replaces the old image and dimensions. | 0 |
| Purge covers | `purge_cover_removes_the_cover_cached_under_the_current_mtime`, `purge_cover_drifted_mtime_removes_nothing_and_does_not_error`, `purge_cover_removes_across_multiple_sides`, `purge_cover_empty_sides_removes_nothing` | keep | Current fingerprint and requested sides select files, while drift and empty requests spare them. | 0 |
| Purge strip | `purge_pages_removes_all_strip_thumbnails_for_a_book`, `purge_pages_overshooting_page_count_skips_missing_keys` | keep | Removes pages for the target and tolerates a count beyond the stored range. | 0 |
| Clear | `clear_missing_dir_returns_zero_report`, `clear_removes_owned_cache_files_and_reports_them`, `clear_keeps_foreign_files_and_nested_entries`, `clear_reclaims_legacy_png` | keep | The removal report counts owned files, including legacy PNGs, and foreign or nested entries survive. | 0 |
| Budget pruning | `prune_missing_dir_returns_zero_report`, `prune_under_cap_removes_nothing`, `prune_over_cap_removes_oldest_first_until_under_cap`, `prune_equal_mtime_tie_breaks_by_file_name`, `prune_zero_cap_removes_all_qois`, `prune_ignores_files_that_are_not_cache_entries` | keep | Fixtures distinguish oldest-first eviction, name ties, exact reporting, missing directories and foreign files. | 0 |
| Temporary file age | `prune_removes_stale_tmp_even_under_cap`, `prune_keeps_fresh_tmp` | strengthen | Survived `thumbnail_cache.rs:214` `age.as_secs() >= TMP_STALE_SECS -> age.as_secs() > TMP_STALE_SECS`. Check exactly the threshold with a deterministic timestamp comparison. | 4-18 |
| Clock skew | `prune_reclaims_tmp_with_mtime_far_in_the_future`, `prune_keeps_tmp_with_mtime_slightly_in_the_future` | keep | A small future offset protects a live write while a much larger offset is reclaimed. | 0 |
| Legacy cleanup | `prune_reclaims_legacy_png_outside_the_cap` | keep | Old PNG files are reclaimed even outside the QOI byte budget. | 0 |
| Touch on hit | `get_hit_advances_file_mtime`, `get_on_corrupt_file_does_not_touch_mtime` | keep | A successful decode refreshes the file timestamp and a corrupt file retains its old timestamp. | 0 |
| Temp file ownership | `is_owned_recognizes_write_atomic_temp_name` | keep | The predicate recognizes the temp-file naming convention used by the atomic writer. | 0 |

## [archive_loader.rs](../crates/gashuu-core/src/archive_loader.rs)

Tests in [archive_loader/tests.rs](../crates/gashuu-core/src/archive_loader/tests.rs).

| Group | Tests | Verdict | Reason / mutation not caught | Est. diff |
| --- | --- | --- | --- | ---: |
| Non-UTF-8 input | `open_with_policy_rejects_a_non_utf8_path`, `probe_page_count_propagates_non_utf8_rejection` | keep | Both native open and probe return the path validation error. | 0 |
| Kind dispatch | `detect_kind_table` | keep | Literal signatures, truncation and the extension/magic conflict pin detection precedence. | 0 |
| In-memory ZIP | `open_read_at_opens_zip_bytes` | keep | The resulting source lists the expected page and reads the fixture bytes. | 0 |
| In-memory unsupported input | `open_read_at_unsupported_carries_name_label` | keep | The error carries the caller-supplied name. | 0 |
| Empty reader probe | `probe_page_count_read_at_empty_zip_is_empty_book_with_name_label` | keep | Zero pages yields EmptyBook with the reader label. | 0 |
| Extension mapping | `kind_from_name_cbr_lowercase_is_rar`, `kind_from_name_rar_lowercase_is_rar`, `kind_from_name_cbr_uppercase_is_rar`, `kind_from_name_cbz_lowercase_is_zip`, `kind_from_name_zip_lowercase_is_zip`, `kind_from_name_txt_is_none`, `kind_from_name_no_extension_is_none` | keep | Known aliases, uppercase and absent or unrelated suffixes have literal expected kinds. | 0 |
| Signature fallback | `detect_reads_magic_for_unknown_extensions` | strengthen | Survived `archive_loader.rs:23` `Rar! -> Bar!` in the RAR signature. Use fixture header bytes independent of the production constants on this route. | 4-18 |
| Native directory open | `directory_with_image_returns_folder_source`, `empty_directory_returns_folder_source_with_no_pages` | keep | Both a populated and an empty directory yield usable folder sources. | 0 |
| ZIP routing | `cbz_extension_opens_zip_source`, `zip_extension_opens_zip_source`, `cbz_extension_case_insensitive`, `no_extension_zip_magic_opens_zip_source`, `spoofed_txt_extension_with_zip_magic_opens_zip_source`, `eocd_magic_no_extension_opens_zip_source_with_empty_pages` | keep | Files open via suffix or magic, including an EOCD-only empty archive. | 0 |
| Native RAR routing | `cbr_extension_opens_rar_source`, `rar_extension_opens_rar_source`, `cbr_extension_uppercase_opens_rar_source`, `spoofed_txt_extension_with_rar_magic_opens_rar_source` | keep | A real archive opens by either alias or signature with the feature enabled. | 0 |
| Unknown native format | `non_zip_txt_returns_unsupported_format`, `non_zip_no_extension_returns_unsupported_format`, `unsupported_format_error_contains_path`, `too_short_file_unknown_extension_returns_unsupported_format` | keep | Short and unrecognized files surface the unsupported-format error with the selected path. | 0 |
| Folder probe | `probe_page_count_folder_with_images_returns_count`, `probe_page_count_folder_images_in_subfolder_only_returns_empty_book`, `probe_page_count_folder_uppercase_extension_is_counted`, `probe_page_count_empty_folder_returns_empty_book` | keep | The count includes uppercase names, excludes child directories and rejects an empty book. | 0 |
| ZIP probe | `probe_page_count_zip_with_image_returns_one`, `probe_page_count_zip_without_images_returns_empty_book` | keep | ZIPs with and without an image distinguish a count from EmptyBook. | 0 |
| Probe errors | `probe_page_count_nonexistent_path_returns_io_error`, `probe_page_count_plain_text_file_returns_unsupported_format` | keep | Missing input and unsupported contents produce different errors. | 0 |
| RAR count | `probe_page_count_cbr_fixture_returns_real_count` | keep | Probes the real fixture and checks its page count. | 0 |
| Build without RAR | `cbr_extension_is_format_disabled_without_rar_feature`, `rar_magic_is_format_disabled_without_rar_feature`, `probe_page_count_cbr_is_format_disabled_without_rar_feature` | keep | Both suffix and signature report disabled format, including the probe route. | 0 |
| Runtime RAR policy | `rar_extension_blocked_when_allow_rar_false`, `cbr_extension_blocked_when_allow_rar_false`, `rar_magic_blocked_when_allow_rar_false`, `zip_still_opens_when_allow_rar_false`, `directory_still_opens_when_allow_rar_false` | keep | Denial applies to aliases and magic while ZIP and folder inputs remain readable. | 0 |

## [library_store.rs](../crates/gashuu-core/src/library_store.rs)

Tests in [library_store/tests.rs](../crates/gashuu-core/src/library_store/tests.rs).

| Group | Tests | Verdict | Reason / mutation not caught | Est. diff |
| --- | --- | --- | --- | ---: |
| Historical library document | `library_json_is_byte_identical_across_bookid_change` | keep | A literal multi-book document survives parsing and a disk save byte for byte. | 0 |
| Current JSON schema | `to_json_emits_version_and_books`, `to_json_emits_positive_page_count_as_bare_integer` | keep | Count zero and a known positive count are bare integers alongside the expected book fields. | 0 |
| Empty document | `from_json_empty_object_yields_empty_library` | keep | The oldest empty object opens as an empty shelf. | 0 |
| Invalid JSON | `from_json_non_object_root_errors`, `from_json_corrupt_text_errors` | keep | Wrong root types and malformed JSON both report library errors. | 0 |
| Future document | `from_json_future_version_errors_without_downgrading`, `load_from_future_version_file_leaves_the_file_untouched` | keep | Rejects unsupported versions and checks the original file bytes after the failed load. | 0 |
| v0 migration | `from_json_migrates_v0_to_current_version` | keep | Materializes the missing shelf data and stamps the current version. | 0 |
| Absent file | `load_from_missing_file_returns_empty_library` | keep | Loading a missing library succeeds with an empty shelf. | 0 |
| Save into missing parents | `save_to_creates_missing_parent_directories` | strengthen | Survived `library_store.rs:102` `self.books() -> &[]`. Save a populated shelf to a new nested path and reload its book. | 4-18 |
| Complete overwrite | `save_to_overwrites_existing_file_with_complete_json` | keep | A shorter second save replaces the entire older document. | 0 |
| Load order | `load_from_normalizes_old_unsorted_library_order` | keep | An unsorted historical document loads in natural title order. | 0 |
| Corrupt file | `load_from_corrupt_file_errors` | keep | The filesystem entry point propagates parse failure. | 0 |
| Quarantine | `library_store_quarantine_renames_aside`, `quarantine_corrupt_file_returns_none_for_missing_source_without_creating_destination` | keep | Renames an existing file and leaves no destination for a missing source. | 0 |
| Override compatibility | `book_without_overrides_loads_as_empty`, `empty_overrides_are_omitted_from_json` | keep | Older books inherit all settings and resaving empty overrides omits the key. | 0 |
| Derived progress | `reading_progress_is_not_persisted` | keep | A save must contain only the persisted progress fields. | 0 |
| Optional bookmark | `old_json_without_last_opened_loads_as_none`, `old_json_without_last_opened_omits_key_on_resave`, `to_json_omits_last_opened_when_none_emits_when_some` | keep | Old documents lack a bookmark and new saves emit the path only when one exists. | 0 |
| Canonical disk round trip | `recanonicalize_round_trips_clean_library_byte_identically` | keep | Loading real, already canonical paths preserves book data and bookmark bytes. | 0 |

## [page_source/mod.rs](../crates/gashuu-core/src/page_source/mod.rs)

| Group | Tests | Verdict | Reason / mutation not caught | Est. diff |
| --- | --- | --- | --- | ---: |
| Mock expectations | `mock_page_source_returns_configured_pages` | delete | Survived `page_source/folder.rs:95` `e.name.clone() -> String::new()` with feature `testing`. Both answers come from test-installed mock closures; no concrete listing or read takes place. | -13 |
| Entry shape | `page_entry_carries_name_only` | keep | An exhaustive literal is a compile-time pin on the name-only public record. | 0 |
| Thread-safety bounds | `page_source_impls_are_send_sync` | keep | Concrete and generated sources must satisfy the thread-safe sharing bounds at compile time. | 0 |

## [page_source/entry_policy.rs](../crates/gashuu-core/src/page_source/entry_policy.rs)

| Group | Tests | Verdict | Reason / mutation not caught | Est. diff |
| --- | --- | --- | --- | ---: |
| Allocation hint | `small_declared_size_is_used_verbatim`, `declared_size_at_the_bound_is_used_verbatim`, `oversized_declared_size_is_clamped_to_the_bound` | property | The hint preserves all declared sizes through the allocation bound and plateaus at that bound for every larger declaration. Returning zero at entry_policy.rs:39 was caught. | 24-45 |
| Preallocation safety margin | `bound_is_far_below_the_entry_ceiling` | strengthen | Survived `page_source/entry_policy.rs:32` `1024 * 1024 -> 499 * 1024 * 1024`. Pin the documented 1 MiB maximum preallocation independently of the entry ceiling. | 4-18 |
| Extension admission | `has_image_ext_recognizes_common_formats`, `has_image_ext_is_case_insensitive`, `has_image_ext_rejects_non_images` | keep | Checks the supported formats, ASCII case variants and near misses such as AVI. | 0 |
| Metadata path | `apple_double_resource_fork_is_metadata`, `macosx_tree_entry_is_metadata`, `ds_store_is_metadata`, `nested_real_image_is_not_metadata`, `plain_image_name_is_not_metadata` | strengthen | Survived `page_source/entry_policy.rs:72` `starts_with_dot \|\| in_macosx -> starts_with_dot`. Include an ordinary basename inside a metadata directory so the predicates are independent. | 8-30 |
| Enclosed name | `nested_relative_path_is_allowed`, `cur_dir_component_is_stripped`, `parent_dir_traversal_is_rejected`, `absolute_path_is_rejected`, `interior_parent_dir_is_rejected` | keep | Ordinary subpaths survive, current-directory components are stripped and absolute or parent components are rejected. | 0 |
| Entry classification | `image_within_size_is_a_page`, `oversized_image_is_a_counted_skip`, `directory_is_ignored`, `non_image_is_ignored`, `macos_metadata_is_ignored_even_when_image_named`, `oversized_non_image_is_ignored_not_skipped` | strengthen | Survived `page_source/entry_policy.rs:104` `is_dir \|\| -> false \|\|`. Give the directory an image extension to distinguish directory rejection from extension rejection. | 8-30 |
| Streaming byte cap | `under_cap_returns_all_bytes`, `cap_or_reject_tests::exactly_at_cap_is_accepted`, `maximum_cap_returns_all_bytes`, `over_cap_rejects_with_entry_too_large` | keep | Tests the inclusive limit, the maximum integer limit and the error payload on overflow. | 0 |
| Materialized byte cap | `under_cap_returns_the_buffer_unchanged`, `reject_over_actual_tests::exactly_at_cap_is_accepted`, `one_over_cap_rejects_with_entry_too_large`, `empty_buffer_is_accepted` | keep | Empty, exact and oversized buffers distinguish the post-read decision and error details. | 0 |

## [page_source/folder.rs](../crates/gashuu-core/src/page_source/folder.rs)

| Group | Tests | Verdict | Reason / mutation not caught | Est. diff |
| --- | --- | --- | --- | ---: |
| Folder inventory | `lists_only_images_in_natural_order`, `macos_metadata_files_are_excluded`, `empty_folder_lists_nothing`, `includes_uppercase_extensions`, `subdirectory_images_are_excluded` | keep | Checks natural order, shallow traversal, file extensions, metadata exclusion and an empty folder using real entries. | 0 |
| Raw folder read | `read_bytes_returns_file_contents`, `read_bytes_out_of_range_errors` | keep | Returns the stored bytes and reports an invalid page index. | 0 |
| File growth | `read_bytes_returns_entry_too_large_when_file_grows_after_open`, `read_file_capped_boundary_is_inclusive` | keep | Applies the inclusive byte limit at read time even if a file grew after listing. | 0 |
| Sorted index-to-file mapping | `read_bytes_matches_list_pages_after_natural_sort` | keep | Distinct files tie each displayed name to its bytes. Reading entry zero at folder.rs:101 was caught. | 0 |

## [page_source/zip.rs](../crates/gashuu-core/src/page_source/zip.rs)

| Group | Tests | Verdict | Reason / mutation not caught | Est. diff |
| --- | --- | --- | --- | ---: |
| Reader-backed open | `from_read_at_matches_open` | strengthen | Survived `page_source/zip.rs:111` `entries.get(index) -> entries.get(0)`. Both routes share the reader. Check the sorted names and distinct bytes against the input fixture. | 4-18 |
| Concurrent reads | `concurrent_read_bytes_from_one_source` | strengthen | Survived `page_source/zip.rs:111` `entries.get(index) -> entries.get(0)`. Precompute each expected buffer from the archive fixture before starting the readers. | 4-18 |
| Persistent archive handle | `read_bytes_does_not_reopen_the_path` | keep | Unlinking the original path must leave the already-open source readable. | 0 |
| ZIP listing | `lists_images_only_flattened_in_natural_order`, `avif_entries_are_indexed_as_pages` | keep | The returned names verify nested image discovery, numeric sorting and AVIF admission. | 0 |
| Complete listing flag | `listing_is_not_truncated_for_a_complete_zip` | keep | A healthy ZIP reports a complete inventory through the source interface. | 0 |
| Skipped count | `macos_metadata_entries_are_excluded_without_counting`, `zip_slip_entry_is_rejected_and_counted`, `unsafe_non_image_entry_is_not_counted_as_skipped` | keep | Metadata and unsafe non-images do not count as lost pages, while an unsafe image does. | 0 |
| Declared ZIP size | `open_size_boundary_is_inclusive`, `open_skips_entry_over_declared_size_limit` | keep | Includes an exact-limit entry and accounts for an oversized declaration during listing. | 0 |
| ZIP random access | `read_bytes_round_trips_and_decodes`, `read_bytes_out_of_range_errors` | keep | A valid index decodes to the expected page and a bad index produces the typed error. | 0 |
| Buffer growth | `read_entry_grows_past_the_initial_capacity_hint` | keep | Reading a larger member returns all its bytes past the initial allocation. | 0 |
| Actual ZIP size | `read_entry_over_actual_size_limit_errors`, `read_entry_size_boundary_is_inclusive` | keep | Independently checks the streaming read ceiling, including equality. | 0 |
| ZIP open errors | `open_corrupt_archive_errors`, `open_missing_file_errors_as_io` | keep | Missing input and corrupt archive contents have distinct error paths. | 0 |

## [page_source/rar.rs](../crates/gashuu-core/src/page_source/rar.rs)

| Group | Tests | Verdict | Reason / mutation not caught | Est. diff |
| --- | --- | --- | --- | ---: |
| RAR inventory | `lists_images_only_flattened_in_natural_order` | keep | The real archive exposes just its images in natural order. | 0 |
| Indexed RAR reads | `read_bytes_round_trips_to_expected_dimensions` | keep | Each page decodes to a different expected shape. Reading entry zero at rar.rs:135 was caught. | 0 |
| Metadata and skipped pages | `macos_metadata_entries_are_excluded_without_counting`, `traversal_entries_are_skipped_and_counted` | keep | Benign metadata and rejected paths affect skipped counts differently. | 0 |
| Partial listing | `corrupt_trailing_header_marks_the_listing_truncated_without_counting_a_skip`, `healthy_archive_is_not_truncated`, `skipped_entries_do_not_imply_truncation` | keep | Distinguishes damaged headers, clean completion and an individually skipped member. | 0 |
| Invalid RAR index | `read_bytes_out_of_range_errors` | keep | Asserts the out-of-range error on the archive implementation. | 0 |
| Declared RAR size | `open_skips_entries_over_declared_size_limit`, `read_entry_over_declared_size_limit_errors` | keep | The listing excludes oversize entries and the read path validates the declaration independently. | 0 |
| RAR open errors | `open_corrupt_archive_errors`, `open_missing_file_errors_as_rar` | keep | Missing and corrupt inputs exercise the RAR backend error contract. | 0 |

## [page_source/read_at.rs](../crates/gashuu-core/src/page_source/read_at.rs)

| Group | Tests | Verdict | Reason / mutation not caught | Est. diff |
| --- | --- | --- | --- | ---: |
| Positional file reads | `file_read_at_concurrent_positional_reads` | keep | Eight readers compare slices against independently generated file bytes. Reading every slice at offset zero at read_at.rs:113 was caught. | 0 |

## [cache.rs](../crates/gashuu-core/src/cache.rs)

Tests in [cache/tests.rs](../crates/gashuu-core/src/cache/tests.rs).

| Group | Tests | Verdict | Reason / mutation not caught | Est. diff |
| --- | --- | --- | --- | ---: |
| Ordered prefetch example | `prefetch_range_in_the_middle` | keep | A literal middle window pins ascending order and omission of the center independently of the property. | 0 |
| Raw read | `read_raw_returns_source_bytes_without_populating_cache` | strengthen | Survived `cache.rs:295` `read_bytes(index) -> read_bytes(0)`. Request a nonzero page with distinct bytes and retain the assertion that reading leaves the cache empty. | 4-18 |
| Raw index error | `read_raw_propagates_source_index_error` | keep | Checks both requested index and source length in the propagated error. | 0 |
| Concurrent insert winner | `insert_decoded_shares_the_cached_arc_and_keeps_the_first_insert` | keep | A second insert returns the first shared allocation without a source read. | 0 |
| Cache hit and lookup | `hit_returns_same_arc_without_rereading`, `get_cached_miss_does_not_read`, `get_cached_hit_returns_same_arc_after_fill` | keep | Pointer identity and read counts distinguish a hit, a miss and a read-free probe. | 0 |
| Dispatch fills shared cache | `dispatch_decode_and_cache_stores_then_get_cached_hits` | strengthen | Survived `cache.rs:295` `read_bytes(index) -> read_bytes(0)`. Verify the bytes of page three as well as the shared allocation in the foreground cache. | 4-18 |
| Decode on miss | `miss_reads_and_decodes` | strengthen | Survived `cache.rs:295` `read_bytes(index) -> read_bytes(0)`. Give the pages distinct pixels and check which page the miss returned. | 4-18 |
| Count eviction | `lru_evicts_oldest_over_capacity`, `lru_hit_promotes_entry_and_evicts_true_lru`, `capacity_is_never_exceeded` | keep | Re-read counts distinguish FIFO from LRU and the resident count respects capacity. | 0 |
| Prefetch reservations | `prefetch_skips_pages_already_in_flight`, `prefetch_skips_cached_and_clears_in_flight` | keep | In-flight or warm pages avoid duplicate reads and completed reservations are cleared. | 0 |
| Prefetch range in cache | `prefetch_warms_neighbours_in_range` | keep | Observes warmed residents as well as the excluded page beyond the radius. | 0 |
| Background decode failure | `prefetch_decode_failure_is_uncached_and_get_surfaces_error` | keep | A failed decode leaves no entry or reservation and a foreground read returns the error. | 0 |
| Byte-budget eviction | `byte_budget_keeps_single_oversized_page_as_floor`, `byte_budget_evicts_through_the_cache` | keep | Pins the oversized-single-page floor and eviction through the cache API. | 0 |

## [image_ops.rs](../crates/gashuu-core/src/image_ops.rs)

| Group | Tests | Verdict | Reason / mutation not caught | Est. diff |
| --- | --- | --- | --- | ---: |
| QOI encoding | `encode_qoi_round_trips_dimensions_and_pixels`, `encode_qoi_rejects_a_short_rgba_buffer` | keep | Magic, dimensions and pixels round-trip, and malformed input reports expected and actual lengths. | 0 |
| RGBA thumbnail | `downscale_to_thumbnail_fits_the_long_edge_and_preserves_aspect`, `downscale_to_thumbnail_keeps_small_images_unchanged` | keep | Literal long-edge sizes and unchanged small-image pixels distinguish shrinking from upscaling. | 0 |
| Sample precision | `decode_thumbnail_resizes_before_quantizing_16_bit_samples` | keep | A 16-bit fixture yields a different pixel if quantization precedes resizing. | 0 |
| Decoded PNG pixels | `decode_reports_dimensions_and_rgba_length` | strengthen | Survived `image_ops.rs:123` `decode_dynamic(bytes)?.into_rgba8() -> image::RgbaImage::new(decode_dynamic(bytes)?.width(), decode_dynamic(bytes)?.height())`. Assert independently known RGBA values as well as dimensions. | 4-18 |
| Malformed page | `decode_invalid_bytes_errors` | keep | Invalid image data produces a decode error. | 0 |
| RGBA validation | `new_rejects_mismatched_rgba_length`, `new_accepts_matching_rgba_length` | keep | Both valid and invalid buffer lengths exercise the constructor invariant. | 0 |
| Pixel budget | `check_pixel_limit_exact_max_is_ok`, `check_pixel_limit_one_over_errors`, `check_pixel_limit_small_size_is_ok` | keep | The exact maximum is allowed and the next pixel count is rejected. | 0 |
| Oversize page header | `decode_rejects_oversized_header_with_image_too_large` | keep | An oversized header yields the size error through the full-page decode entry point. | 0 |
| Thumbnail without upscaling | `decode_thumbnail_does_not_upscale`, `decode_thumbnail_jpeg_does_not_upscale` | keep | Small PNG and JPEG fixtures retain their original dimensions. | 0 |
| Malformed thumbnail | `decode_thumbnail_invalid_bytes_errors` | keep | Invalid bytes reach the thumbnail decode error path. | 0 |
| AVIF availability | `decode_avif_without_feature_is_an_error_not_a_panic`, `decode_avif_reports_dimensions_and_rgba_length` | keep | Feature-on decoding exposes the fixture dimensions and the feature-off path returns an error. | 0 |
| Oversize thumbnail header | `decode_thumbnail_rejects_oversized_ihdr_with_image_too_large` | keep | A small requested thumbnail cannot bypass the input pixel ceiling. | 0 |

## [reading_progress.rs](../crates/gashuu-core/src/reading_progress.rs)

| Group | Tests | Verdict | Reason / mutation not caught | Est. diff |
| --- | --- | --- | --- | ---: |
| Recount | `with_total_reclamps_position` | keep | A replacement total clamps an existing last-viewed position. | 0 |
| Change position | `with_last_viewed_clamps_against_known_total`, `with_last_viewed_is_verbatim_when_total_unknown` | keep | A known count clamps a new position while an unknown count preserves it. | 0 |
| Default | `default_is_unread_unknown_total` | keep | A fresh progress value represents page zero with unknown total. | 0 |
| Stored progress | `total_count_round_trips_the_field`, `getters_return_constructed_values` | keep | Reading back distinct position and optional count values protects the persistence-facing accessors. | 0 |
| Fraction | `fraction_total_none_is_always_zero`, `fraction_unread`, `fraction_partway`, `fraction_last_page_is_one`, `fraction_overshoot_clamps_to_one`, `fraction_one_page_total_is_zero` | property | For bounded totals above one progress advances in equal steps from zero to one and stays at one past the end, while unknown or singleton totals yield zero. A constant zero at reading_progress.rs:69 was caught. | 48-90 |
| Completion | `is_finished_matrix` | keep | The last page finishes a multi-page book and a singleton never counts as finished. | 0 |
| Display numbering | `current_at_zero_is_one`, `current_partway`, `current_saturates_at_usize_max` | keep | Page zero displays as one and the largest unknown-total position saturates. A constant one at reading_progress.rs:58 was caught. | 0 |
| Construction | `new_clamps_position_into_the_book`, `new_leaves_position_verbatim_when_total_unknown` | keep | A known, shrunken book clamps stale progress while an unknown count preserves it verbatim. | 0 |
| Display bounds | `current_never_exceeds_total` | property | For every known nonzero total and requested position the displayed page lies between one and the total, inclusive. Removing the constructor clamp at reading_progress.rs:33 was caught. | 20-40 |
| At start | `is_at_start_true_when_last_viewed_zero`, `is_at_start_false_when_last_viewed_nonzero` | keep | Starting position depends on the recorded page being zero. | 0 |

## [crates/gashuu-worker/src/tests.rs](../crates/gashuu-worker/src/tests.rs)

| Group | Tests | Verdict | Reason / mutation not caught | Est. diff |
| --- | --- | --- | --- | ---: |
| Request metadata | `all_requests_round_trip` | strengthen | Survived `protocol.rs:20` `tag = "t" -> tag = "kind"`. Decode a literal request envelope and check the discriminator the sender writes. | 4-18 |
| Response metadata | `all_responses_round_trip` | strengthen | Survived `protocol.rs:77` `tag = "t" -> tag = "kind"`. Check a literal response envelope independently of the derived decoder. | 4-18 |
| Malformed protocol | `malformed_json_is_a_protocol_error` | keep | Invalid JSON maps to the protocol error. | 0 |
| Book path | `parses_valid_book_paths`, `rejects_invalid_book_paths` | keep | Accepts the stored-book path shape and rejects malformed components with the original ID. | 0 |
| Open reply | `open_reports_count_skipped_and_echoes_id`, `open_of_zero_image_zip_is_ok_with_zero_pages` | keep | The response echoes the request ID and actual listing counts, including a zero-page ZIP. | 0 |
| Spread reply | `decode_spread_returns_both_images_in_one_reply`, `decode_spread_trailing_error_keeps_leading` | keep | Checks buffer order, slot metadata and delivery of the leading page when the trailing page fails. | 0 |
| Fallback decoder | `primary_failure_retries_the_same_bytes_with_the_fallback`, `primary_success_does_not_call_the_fallback_hook`, `both_decoders_failing_is_a_per_slot_error` | keep | A failing primary retries identical bytes, a successful one bypasses fallback, and two failures remain per-slot errors. | 0 |
| Decoded-page lifetime | `worker_keeps_no_decoded_pages` | keep | A repeated request invokes the decoder again, pinning the worker memory policy. | 0 |
| Thumbnail and cover sizing | `thumbnail_and_cover_downscale_the_primary_decode_to_max_side` | keep | Both dispatcher routes deliver the scaled dimensions from the primary decoder. | 0 |
| Request priority | `lane_of_each_request_kind` | keep | Scheduling lanes are externally meaningful for foreground turns and background work. | 0 |
| Close control | `close_is_out_of_band`, `closed_session_discards_open_and_marks_thumbnail_obsolete`, `close_unknown_session_is_noop` | keep | Closing invalidates queued work and releases readers without invalidating another live session. | 0 |
| Cover generation | `cancel_covers_marks_older_generations_obsolete` | keep | A lower cancellation threshold cannot revive obsolete cover requests. | 0 |
| Cover reply sequence | `cover_without_count_sends_one_reply`, `cover_with_count_sends_cover_then_count`, `cover_of_empty_book_is_empty_book_without_count` | keep | The flag controls the second count reply and empty books return a single failure. | 0 |
| Empty probe | `probe_page_count_of_empty_zip_is_empty_book` | keep | Probing a zero-page archive returns EmptyBook. | 0 |
| Archive request failures | `archive_requests_reject_invalid_ids`, `archive_requests_require_a_reader`, `archive_requests_preserve_a_missing_reader_error`, `archive_requests_report_a_corrupt_zip` | keep | Exercises each dispatcher route with invalid IDs, absent readers, specific reader errors and corrupt bytes. | 0 |
| Reader lifetime | `reader_is_shared_and_released_when_last_session_closes` | keep | Two sessions share one reader until the last one closes. | 0 |
| Unknown session | `unknown_session_is_rejected`, `thumbnail_for_unknown_session_is_rejected` | keep | Spread and thumbnail requests return an error for a missing session. | 0 |
| Close during decode | `closing_during_decode_discards_the_pending_reply`, `closing_during_thumbnail_decode_discards_the_pending_reply`, `closing_during_trailing_decode_discards_the_pending_spread` | keep | Polls pending work, closes its session and verifies the completed work emits no reply, including during the trailing slot. | 0 |
| Cancel during cover decode | `cancelling_during_cover_decode_discards_pixels_and_count` | keep | A cancellation between polls suppresses both pixels and count. | 0 |
| Filename examples | `import_names_are_safe_and_keep_the_archive_extension`, `sanitise_truncated_name_without_extension_has_no_trailing_whitespace`, `sanitise_truncated_stem_with_extension_has_no_trailing_whitespace` | keep | Literal results pin separators, multibyte truncation, fallback names and whitespace beside an extension. | 0 |
| Usable upload basename | `sanitise_upload_name_is_idempotent_on_existing_cases`, `sanitised_name_parses_as_a_stored_book_id` | property | For any upload name, sanitizing twice gives the same basename and that basename forms a parseable stored-book ID. Returning an empty extensionless result at import.rs:38 was caught by the group. | 20-40 |
| Desktop title rule | `upload_titles_follow_the_desktop_file_rule` | strengthen | Survived `crates/gashuu-core/src/library.rs:98` `path.to_string_lossy().into_owned() -> String::new()`. Assert literal fallback titles, especially for the dot path, alongside the ordinary archive stem. | 4-18 |
| Quota preflight | `quota_preflight_accounts_for_usage_and_unknown_estimates` | keep | Equality, insufficient space and missing estimates have explicit decisions independent of monotonicity. | 0 |
| Import metadata | `import_protocol_round_trips_metadata_without_file_bytes` | keep | Pins the metadata-only request shape, background dispatch and import result variants. | 0 |
| Disguised RAR | `import_rejects_a_rar_renamed_as_cbz` | keep | Checks the actual signature when the upload suffix says ZIP. | 0 |
| Staged cover operations | `cover_prune_and_staged_store_contract` | keep | Staged writes reload real pixels, purge records the key and the clear flag is consumed once. | 0 |
| Cache-hit cover reply | `cover_store_hit_skips_decode_and_still_counts` | keep | Reuse bypasses decoding without losing the optional page-count reply. | 0 |
| Selective staged purge | `staged_purge_only_removes_selected_book_and_bad_data_misses` | keep | A different book survives removal and corrupt cached data gives a miss. | 0 |
| Cache-write failure | `failed_cover_write_does_not_prevent_reply` | keep | A failed store write still delivers a successful cover response. | 0 |
| Upload sampling | `upload_fingerprint_and_ranges` | property | For every file size, each sample contains the entire file up to 1 MiB and otherwise exactly 1 MiB, with one anchored at the beginning and one at the end. Returning zero sample length at lifecycle.rs:45 was caught. Retain the literal SHA-256 vector in this test. | 20-40 |
| Content-key syntax | `validates_content_key` | keep | Rejects uppercase and incorrect lengths even for otherwise hexadecimal keys. | 0 |
| Content index recovery | `content_index_remains_usable_after_a_stored_key_error` | keep | One unreadable stored key leaves valid entries on either side usable. | 0 |
| Sweep inventory | `startup_and_clear_sweep`, `startup_sweep_retains_fileless_directories_with_recent_or_unknown_age`, `sweep_inventory_includes_locked_books_and_counts_empty_locked_directories` | keep | Keep lists, age, lock state and empty directories affect the concrete deletion plan and counts. | 0 |

## Proposed follow-up issues

Each proposal fits in one test PR and depends on #758. They can proceed independently. The scope of each is the non-keep rows in the named files.

1. `[1/9] test(core): check spread navigation against visible pages` — Use literal spreads for context and round-trip checks, then generate turn and layout sequences for the navigation invariants.
2. `[2/9] test(core): compare batch shelf insertion and verify nested saves` — Generate duplicate and pre-existing IDs for the bulk-add comparison. Reload a populated library from a newly created directory.
3. `[3/9] test(core): verify the version and payload of a settings save` — Exercise non-default settings through normalization and serialization together.
4. `[4/9] test(core): pin the temporary thumbnail expiry boundary` — Check equality at the stale threshold without a clock race or a sleep.
5. `[5/9] test(core): verify archive detection and indexed page-source results` — Use independent magic bytes and ZIP contents, isolate directory and metadata predicates, verify allocation limits and remove the mock-only smoke test.
6. `[6/9] test(core): identify decoded cache misses by page content` — Make the counting source distinguish page indices and check both payload and cache effects.
7. `[7/9] test(core): verify the pixels returned by PNG decoding` — Assert known RGBA bytes for a lossless fixture.
8. `[8/9] test(core): generate reading progress and display bounds` — Generate bounded progress inputs with explicit endpoints and preserve the unknown-total and single-page rules.
9. `[9/9] test(core): pin worker wire tags and upload behavior` — Add literal request and response metadata and fallback titles. Generate upload names and sample ranges, retaining the fixed fingerprint vector.
