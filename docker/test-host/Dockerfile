FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    openssh-server \
    tmux \
    coreutils \
    && rm -rf /var/lib/apt/lists/*

# Create test user
RUN useradd -m -s /bin/bash testuser && \
    mkdir -p /home/testuser/.ssh && \
    chmod 700 /home/testuser/.ssh && \
    chown -R testuser:testuser /home/testuser

# Install test identity public key
COPY test_ed25519.pub /home/testuser/.ssh/authorized_keys
RUN chmod 600 /home/testuser/.ssh/authorized_keys && \
    chown testuser:testuser /home/testuser/.ssh/authorized_keys

# Configure sshd
COPY sshd_config /etc/ssh/sshd_config.d/mux-test.conf
RUN ssh-keygen -A && \
    mkdir -p /var/run/sshd

EXPOSE 22

CMD ["/usr/sbin/sshd", "-D", "-e"]
