"""The sandbox as the agent finds it: the session's files under /mnt/session/uploads, its agents'
skills under /workspace/skills, the egress CA trusted, and outputs written back as session files."""

import hashlib
import io
import json
import os
import zipfile
from pathlib import Path

from .prompt import SKILLS_DIR, skill_description

UPLOADS = Path("/mnt/session/uploads")
OUTPUTS = Path("/mnt/session/outputs")
CA_PATH = Path("/etc/agentd/egress-ca.pem")
CA_BUNDLE = Path("/tmp/agentd/ca-bundle.pem")
SYSTEM_CAS = Path("/etc/ssl/certs/ca-certificates.crt")
STATE = Path("/var/lib/agentd/outputs.json")
NOBODY = 65534


def trust_ca(pem):
    """Write the session CA, and the bundle the sandbox's tools point at: the system CAs and it."""
    if not pem:
        return
    CA_PATH.parent.mkdir(parents=True, exist_ok=True)
    CA_PATH.write_text(pem)
    CA_BUNDLE.parent.mkdir(parents=True, exist_ok=True)
    system = SYSTEM_CAS.read_text() if SYSTEM_CAS.exists() else ""
    CA_BUNDLE.write_text(system + "\n" + pem)
    os.chmod(CA_BUNDLE, 0o644)


def upload_path(file_id, mount_path):
    rel = (mount_path or file_id).lstrip("/")
    if rel.startswith("mnt/session/uploads/"):
        rel = rel[len("mnt/session/uploads/"):]
    return UPLOADS / rel


def mount_files(agentd, session):
    for resource in session.get("resources") or []:
        if resource.get("type") != "file":
            continue
        target = upload_path(resource["file_id"], resource.get("mount_path"))
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(agentd.file(resource["file_id"]))
        os.chmod(target, 0o444)


def unpack_skill(archive, skills_dir=Path(SKILLS_DIR)):
    """Extract a skill archive into its own directory; its name and description."""
    with zipfile.ZipFile(io.BytesIO(archive)) as z:
        names = [n for n in z.namelist() if not n.endswith("/")]
        top = {n.split("/", 1)[0] for n in names}
        wrapped = len(top) == 1 and all("/" in n for n in names)
        md = next((n for n in names if n.rsplit("/", 1)[-1] == "SKILL.md"), None)
        text = z.read(md).decode("utf-8", "replace") if md else ""
        # The archive's top directory names the skill, as agentd's own bundles do.
        name = top.pop() if wrapped else next(
            (l.split(":", 1)[1].strip().strip('"') for l in text.split("\n---", 1)[0].splitlines() if l.startswith("name:")),
            "skill")
        dest = skills_dir / name
        for n in names:
            rel = n.split("/", 1)[1] if wrapped else n
            if ".." in Path(rel).parts:
                continue
            out = dest / rel
            out.parent.mkdir(parents=True, exist_ok=True)
            out.write_bytes(z.read(n))
    return name, skill_description(text)


def mount_skills(agentd, agents):
    """Unpack every agent's skills once; each agent's (name, description) list."""
    done, per_agent = {}, {}
    for agent in agents:
        infos = []
        for skill in agent.get("skills") or []:
            # Anthropic's prebuilt skills are not agentd's to serve.
            if skill.get("type") != "custom":
                continue
            key = (skill["skill_id"], str(skill.get("version") or "latest"))
            if key not in done:
                done[key] = unpack_skill(agentd.skill(skill["skill_id"], key[1]))
            infos.append(done[key])
        per_agent[agent["name"]] = infos
    return per_agent


def prepare_dirs():
    for d in (Path("/workspace"), OUTPUTS):
        d.mkdir(parents=True, exist_ok=True)
        os.chown(d, NOBODY, NOBODY) if os.geteuid() == 0 else None


def sync_outputs(agentd, root=OUTPUTS, state=STATE):
    """Save outputs that changed since the last sync as session files, one file per path."""
    synced = json.loads(state.read_text()) if state.exists() else {}
    count = 0
    for path in sorted(p for p in root.rglob("*") if p.is_file()):
        rel = str(path.relative_to(root))
        data = path.read_bytes()
        digest = hashlib.sha256(data).hexdigest()
        known = synced.get(rel)
        if known and known["sha256"] == digest:
            continue
        file_id = agentd.save_output(rel, data, previous=known and known["file_id"])
        synced[rel] = {"sha256": digest, "file_id": file_id}
        count += 1
    state.parent.mkdir(parents=True, exist_ok=True)
    state.write_text(json.dumps(synced))
    return count
