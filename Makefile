VENV_PYTHON = /home/ht/docker/pysim/venv/bin/python3

.PHONY: setup setup-watch watch list \
        compose build-images \
	verify-fix \
        logs-% exec-% \
        stop clean

# ── SIM management ───────────────────────────────────────────────────────────

setup:
	$(VENV_PYTHON) start_sim_container_config.py --setup

setup-watch:
	$(VENV_PYTHON) start_sim_container_config.py --setup --watch

watch:
	$(VENV_PYTHON) start_sim_container_config.py --watch

list:
	$(VENV_PYTHON) start_sim_container_config.py --list

# ── Docker / compose ─────────────────────────────────────────────────────────

compose:
	$(VENV_PYTHON) start_sim_container_config.py --setup

build-images:
	./build-images.sh

verify-fix:
	cd sms-gateway && ./tools/verify-fix.sh

stop:
	docker compose stop

clean:
	docker compose down -v --remove-orphans 2>/dev/null || true
	rm -rf config/[1-9]/ _backup/ ../logs/
	$(VENV_PYTHON) -c "from scripts.sim_db import DB_PATH; import os; os.remove(DB_PATH) if os.path.exists(DB_PATH) else None" 2>/dev/null || true
	@echo "Cleaned"

# ── Convenience: logs / exec ────────────────────────────────────────────────

logs-%:
	docker compose logs -f --tail=20 $(subst logs-,,$@)

exec-%:
	docker compose exec $(subst exec-,,$@) /bin/sh
