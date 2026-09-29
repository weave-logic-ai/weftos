"""Binary cache and runtime adapters for the cog conformance harness.

An adapter takes a prepared work directory (harness.py + plan.json + bin/)
and produces results.json in it by running harness.py on the target:

  docker          `docker run --platform linux/<arch>` (OrbStack, Docker Engine / Desktop)
  apple-container Apple `container run --arch <arch>` (macOS 26+)
  native          python3 harness.py on this host (a Linux ARM node, run as root
                  or with CAP_NET_BIND_SERVICE: cogs post to 127.0.0.1:80)
  ssh             copy the work dir to a remote node and run it natively there
                  (remote-node mode; the host comes from --ssh-host or
                  COG_HARNESS_SSH_HOST and is never written into results)
"""
import os
import shlex
import shutil
import subprocess
import urllib.error
import urllib.request

from classify import valid_cog_id

BASE_URL = "https://storage.googleapis.com/cognitum-apps/cogs"
URL_ARCH = {"aarch64": ("arm64", "aarch64"), "arm": ("arm", "arm")}
ELF_MACHINE = {"aarch64": 183, "arm": 40}   # EM_AARCH64, EM_ARM
DOCKER_PLATFORM = {"aarch64": "linux/arm64", "arm": "linux/arm/v7"}
CONTAINER_ARCH = {"aarch64": "arm64", "arm": "arm"}
DEFAULT_IMAGE = "python:3.12-slim-bookworm"
MAX_BINARY = 64 << 20


def binary_name(cid, arch):
    return "cog-%s-%s" % (cid, URL_ARCH[arch][1])


def binary_url(cid, arch, base=BASE_URL):
    if not valid_cog_id(cid):
        raise ValueError("invalid cog id %r" % cid)
    if arch not in URL_ARCH:
        raise ValueError("arch must be one of %s" % sorted(URL_ARCH))
    return "%s/%s/%s" % (base.rstrip("/"), URL_ARCH[arch][0], binary_name(cid, arch))


def check_elf(data, arch):
    """Return None if `data` looks like a Linux ELF for `arch`, else a reason."""
    if len(data) < 20 or data[:4] != b"\x7fELF":
        return "not an ELF file"
    little = data[5] == 1
    machine = int.from_bytes(data[18:20], "little" if little else "big")
    if machine != ELF_MACHINE[arch]:
        return "ELF machine %d is not %s" % (machine, arch)
    return None


def fetch_binary(cid, arch, cache_dir, base=BASE_URL, opener=None):
    """Return (path, None) for a cached/downloaded binary, or (None, reason)."""
    os.makedirs(cache_dir, exist_ok=True)
    path = os.path.join(cache_dir, binary_name(cid, arch))
    if os.path.isfile(path):
        with open(path, "rb") as f:
            head = f.read(64)
        if check_elf(head, arch) is None:
            return path, None
        os.remove(path)
    url = binary_url(cid, arch, base)
    opener = opener or urllib.request.urlopen
    try:
        with opener(url, timeout=60) as resp:
            data = resp.read(MAX_BINARY + 1)
    except urllib.error.HTTPError as e:
        return None, "HTTP %d" % e.code
    except (urllib.error.URLError, OSError) as e:
        return None, "fetch failed: %s" % e
    if len(data) > MAX_BINARY:
        return None, "binary larger than %d bytes" % MAX_BINARY
    why = check_elf(data, arch)
    if why:
        return None, why
    tmp = path + ".part"
    with open(tmp, "wb") as f:
        f.write(data)
    os.chmod(tmp, 0o755)
    os.replace(tmp, path)
    return path, None


# ── Adapters ────────────────────────────────────────────────────────────────

class Adapter:
    name = "base"

    def __init__(self, arch, image=DEFAULT_IMAGE, runner=subprocess.run):
        if arch not in URL_ARCH:
            raise ValueError("arch must be one of %s" % sorted(URL_ARCH))
        self.arch, self.image, self.runner = arch, image, runner

    def binary_root(self, workdir):
        """Path prefix under which the target sees workdir."""
        return "/w"

    def commands(self, workdir):
        raise NotImplementedError

    def run(self, workdir, timeout):
        for cmd in self.commands(workdir):
            p = self.runner(cmd, timeout=timeout)
            if p.returncode != 0:
                raise RuntimeError("%s adapter: %s exited %s" % (
                    self.name, shlex.join(cmd[:3]), p.returncode))
        return os.path.join(workdir, "results.json")


def _in_target(root):
    return ["python3", root + "/harness.py", "--plan", root + "/plan.json",
            "--out", root + "/results.json"]


class DockerAdapter(Adapter):
    name = "docker"

    def commands(self, workdir):
        return [["docker", "run", "--rm", "--platform", DOCKER_PLATFORM[self.arch],
                 "-v", "%s:/w" % os.path.abspath(workdir), self.image] + _in_target("/w")]


class AppleContainerAdapter(Adapter):
    name = "apple-container"

    def commands(self, workdir):
        return [["container", "run", "--rm", "--arch", CONTAINER_ARCH[self.arch],
                 "-v", "%s:/w" % os.path.abspath(workdir), self.image] + _in_target("/w")]


class NativeAdapter(Adapter):
    name = "native"

    def binary_root(self, workdir):
        return os.path.abspath(workdir)

    def commands(self, workdir):
        return [_in_target(os.path.abspath(workdir))]


class SshAdapter(Adapter):
    """Remote-node mode: scp the work dir, run natively (optionally via sudo)."""
    name = "ssh"
    REMOTE_DIR = "cog-conformance"

    def __init__(self, arch, host, sudo=False, **kw):
        super().__init__(arch, **kw)
        if not host or host.startswith("-") or any(c.isspace() for c in host):
            raise ValueError("ssh host must be a plain [user@]host alias")
        self.host, self.sudo = host, sudo

    def binary_root(self, workdir):
        return self.REMOTE_DIR  # relative to the remote login directory

    def commands(self, workdir):
        rd = self.REMOTE_DIR
        run = _in_target(rd)
        if self.sudo:
            run = ["sudo", "-n"] + run
        return [
            ["ssh", self.host, "rm -rf %s && mkdir -p %s" % (rd, rd)],
            ["scp", "-q", "-r"] + [os.path.join(os.path.abspath(workdir), n)
                                    for n in ("harness.py", "plan.json", "bin")]
            + ["%s:%s/" % (self.host, rd)],
            ["ssh", self.host, shlex.join(run)],
            ["scp", "-q", "%s:%s/results.json" % (self.host, rd),
             os.path.join(os.path.abspath(workdir), "results.json")],
        ]


ADAPTERS = {"docker": DockerAdapter, "apple-container": AppleContainerAdapter,
            "native": NativeAdapter, "ssh": SshAdapter}


def make_adapter(runtime, arch, ssh_host=None, sudo=False, image=DEFAULT_IMAGE,
                 runner=subprocess.run):
    if runtime not in ADAPTERS:
        raise ValueError("runtime must be one of %s" % sorted(ADAPTERS))
    if runtime == "ssh":
        host = ssh_host or os.environ.get("COG_HARNESS_SSH_HOST")
        return SshAdapter(arch, host, sudo=sudo, image=image, runner=runner)
    return ADAPTERS[runtime](arch, image=image, runner=runner)


def stage_workdir(workdir, harness_src, binaries):
    """Create workdir/{harness.py,bin/} with hard links (or copies) of binaries."""
    os.makedirs(os.path.join(workdir, "bin"), exist_ok=True)
    shutil.copy2(harness_src, os.path.join(workdir, "harness.py"))
    for src in binaries:
        dst = os.path.join(workdir, "bin", os.path.basename(src))
        if os.path.exists(dst):
            os.remove(dst)
        try:
            os.link(src, dst)
        except OSError:
            shutil.copy2(src, dst)
