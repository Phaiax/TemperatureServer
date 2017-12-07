#!/bin/bash


SERVICENAME=temperatures
EXECUTABLE=$(pwd)/target/debug/server
SERVICEFILE=/etc/systemd/system/${SERVICENAME}.service
USER=$USER
#LOGDIRECTORY=${HOME}/log
#[Service] Environment="LOGDIRECTORY=${LOGDIRECTORY}"

echo "** MAKE ${LOGDIRECTORY}"

mkdir -p $LOGDIRECTORY

echo "** WRITE SERVICE FILE to $SERVICEFILE"

sudo tee $SERVICEFILE <<EOF
[Unit]
Description=${SERVICENAME} Service
After=network.target

[Service]
Type=simple
User=${USER}
ExecStart=${EXECUTABLE}
Restart=on-abort

[Install]
WantedBy=multi-user.target
EOF

echo "** RELOAD SYSTEMD"

sudo systemctl daemon-reload

echo "** TRY STOP SERVICE"

sudo systemctl status $SERVICENAME | grep ' active '
## if found, $? == 0
OUT=$?
if [ $OUT -eq 0 ]; then
    sudo systemctl stop $SERVICENAME
fi

echo "** ENABLE SERVICE FOR AUTOSTART"

sudo systemctl enable $SERVICENAME

echo "** START SERVICE"

sudo systemctl start $SERVICENAME

echo "** SERVICE STATUS"

sudo systemctl status $SERVICENAME
