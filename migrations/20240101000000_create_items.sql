-- migrations/20240101000000_create_items.sql
-- Migration: สร้าง items table
--
-- หลักการ:
-- 1. ทุก table มี id (UUID as TEXT ใน SQLite), created_at, updated_at
-- 2. ใช้ deleted_at สำหรับ soft delete (ไม่ลบจริง)
-- 3. สร้าง index บน column ที่ใช้ filter/sort บ่อย

CREATE TABLE IF NOT EXISTS items (
    id          TEXT        NOT NULL PRIMARY KEY,
    name        TEXT        NOT NULL,
    description TEXT,
    created_at  TEXT        NOT NULL,   -- ISO 8601 datetime
    updated_at  TEXT        NOT NULL,   -- ISO 8601 datetime
    deleted_at  TEXT                    -- NULL = active, non-NULL = soft deleted
);

-- Index สำหรับ filter active items
CREATE INDEX IF NOT EXISTS idx_items_deleted_at
    ON items (deleted_at);

-- Index สำหรับ sort by created_at DESC (default sort)
CREATE INDEX IF NOT EXISTS idx_items_created_at
    ON items (created_at DESC)
    WHERE deleted_at IS NULL;
