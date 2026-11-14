#!/bin/sh
# Prints the STRUCTURE of an Apple Photos library database, to help build the
# "import from Apple Photos" feature. It only reads, and it prints no photo
# names, album names, dates or places: just table layouts and row counts.
#
# Usage:
#   sh tools/photos-schema.sh                       # the default library
#   sh tools/photos-schema.sh "/path/to/My.photoslibrary"
#
# If it says "unable to open database" or "authorization denied", macOS is
# protecting the Pictures folder: give your terminal app "Full Disk Access"
# in System Settings > Privacy & Security, then run it again.

LIBRARY="${1:-$HOME/Pictures/Photos Library.photoslibrary}"
DB="$LIBRARY/database/Photos.sqlite"

if [ ! -f "$DB" ]; then
  echo "No database at: $DB" >&2
  exit 1
fi

# "mode=ro" opens the database read-only: nothing in the library is changed.
q() {
  sqlite3 "file:$DB?mode=ro" "$1" 2>&1
}

echo "== System"
sw_vers 2>/dev/null
sqlite3 --version

echo
echo "== Entities (table numbers used in the join tables)"
q "SELECT Z_ENT, Z_NAME FROM Z_PRIMARYKEY ORDER BY Z_ENT;"

echo
echo "== Tables"
q "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name;"

echo
echo "== Columns of the tables the import reads"
for table in ZASSET ZADDITIONALASSETATTRIBUTES ZGENERICALBUM ZINTERNALRESOURCE; do
  echo "-- $table"
  q "SELECT name || ' ' || type FROM pragma_table_info('$table');"
done

echo
echo "== Columns of the album-to-asset join tables"
for table in $(q "SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'Z\_%ASSETS' ESCAPE '\\' ORDER BY name;"); do
  echo "-- $table"
  q "SELECT name || ' ' || type FROM pragma_table_info('$table');"
  q "SELECT 'rows: ' || COUNT(*) FROM $table;"
done

echo
echo "== Counts (no names)"
q "SELECT 'assets: ' || COUNT(*) FROM ZASSET;"
q "SELECT 'assets by trashed state: ' || ZTRASHEDSTATE || ' -> ' || COUNT(*) FROM ZASSET GROUP BY ZTRASHEDSTATE;"
q "SELECT 'favorites: ' || COUNT(*) FROM ZASSET WHERE ZFAVORITE = 1;"
q "SELECT 'hidden: ' || COUNT(*) FROM ZASSET WHERE ZHIDDEN = 1;"
q "SELECT 'albums of kind ' || ZKIND || ': ' || COUNT(*) FROM ZGENERICALBUM GROUP BY ZKIND;"
q "SELECT 'assets with an original file name: ' || COUNT(*) FROM ZADDITIONALASSETATTRIBUTES WHERE ZORIGINALFILENAME IS NOT NULL;"
q "SELECT 'assets with an original file size: ' || COUNT(*) FROM ZADDITIONALASSETATTRIBUTES WHERE ZORIGINALFILESIZE > 0;"

echo
echo "== How the original files are stored (folder depth and extensions only)"
q "SELECT 'directory length ' || LENGTH(ZDIRECTORY) || ', extension ' || LOWER(SUBSTR(ZFILENAME, INSTR(ZFILENAME, '.'))) || ': ' || COUNT(*) FROM ZASSET GROUP BY 1 ORDER BY COUNT(*) DESC LIMIT 12;"
ls "$LIBRARY" 2>&1 | head -20
