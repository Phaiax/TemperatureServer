

import numpy as np

import matplotlib.pyplot as plt

import msgpack

from os import listdir
from os.path import isfile, join



coefficients_outside = [5.49063589e-03, -4.51545766e-04, 2.00477893e-06]
onehundreds_at0deg = [99700, 101200, 102200, 102900, 102900, 98600]
R_ref_outside = onehundreds_at0deg[5]


def recalc(raw, coeffs, R_ref):
    Rs = (raw * R_ref) / (1023 - raw)
    Tinv = coeffs[0] + coeffs[1] * np.log(Rs) + coeffs[2] * (np.log(Rs))**3
    Ts = (1/Tinv) - 273.15
    return int(Ts * 100)


filedbpath = "/home/daniel/tmp/tempcalib/log"
filedbpath_recalculated = "/home/daniel/tmp/tempcalib/log_new"

db_files = [f for f in listdir(filedbpath) if isfile(join(filedbpath, f)) and f.endswith(".db.v2")]

f = db_files[0]
db_files.sort()

print("Use {} database files.".format(len(db_files)))
# print(db_files)

total_entries = 0
data = []

for db_file in db_files:
    with open(join(filedbpath, db_file), "r") as orig:
        with open(join(filedbpath_recalculated, db_file), "w") as recalced:
            unpacker = msgpack.Unpacker(orig)
            logdata = unpacker.__next__()

            for i in range(0, len(logdata)):
                logdata[i][1][1][5] = recalc(logdata[i][1][0][5], coefficients_outside, R_ref_outside)

            msgpack.pack(logdata, recalced)

print("Ready")


