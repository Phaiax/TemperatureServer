
import numpy as np

import matplotlib.pyplot as plt



#
# -1.6  ,  720.8   720.6   633.6   590.2   624.7   691.0
# 16.2  ,  590.2   595.6   539.1   513.8   537.5   585.0
# -18.5 ,  811.2   806.7   698.4   650.6   692.9   771.1
# 2.4   ,  700.0   701.1   620.1   580.9   614.5   678.8

# temps = np.array([-1.6, 16.2, -18.5, 2.4])
# sensors_raws = [ np.array([720.8 ,590.2 ,811.2 ,700.0]),
#                  np.array([720.6, 595.6, 806.7, 701.1]),
#                  np.array([633.6, 539.1, 698.4, 620.1]),
#                  np.array([590.2, 513.8, 650.6, 580.9]),
#                  np.array([624.7, 537.5, 692.9, 614.5]),
#                  np.array([691.0, 585.0, 771.1, 678.8]) ]
# weights = [1, 1, 1, 1] #weight of measurement
[ 691.4, 693.9, 615.9, 579.8, 612.5, 672.5 ]
[ 677.4, 680.6, 606.5, 569.9, 600.1, 659.9 ]
[ 809.6, 806.3, 702.5, 650.1, 688.4, 766.5 ]
[ 813.7, 811.5, 697.6, 646.6, 683.9, 761.6 ]
[ 702.6, 700.8, 622.5, 581.1, 615.9, 680.0 ]
[ 701.7, 700.2, 621.5, 580.8, 615.3, 679.5 ]
[ 595.4, 599.9, 542.3, 516.4, 541.6, 592.1 ]



temps = np.array([4.81, 5.93, -14.31, -17.74, 1.93, 2.06, 15.87])
sensors_raws = [ np.array([691.4, 677.4, 809.6, 813.7, 702.6, 701.7, 595.4]),
                 np.array([693.9, 680.6, 806.3, 811.5, 700.8, 700.2, 599.9]),
                 np.array([615.9, 606.5, 702.5, 697.6, 622.5, 621.5, 542.3]),
                 np.array([579.8, 569.9, 650.1, 646.6, 581.1, 580.8, 516.4]),
                 np.array([612.5, 600.1, 688.4, 683.9, 615.9, 615.3, 541.6]),
                 np.array([672.5, 659.9, 766.5, 761.6, 680.0, 679.5, 592.1]) ]
weights = [1, 1, 100, 0.002, 1, 0.2, 1] #weight of measurement

# the 100ks # A7..A2 #order is ok
onehundreds_at0deg = [99700, 101200, 102200, 102900, 102900, 98600]
onehundreds_at25deg = [9900, 100300, 101200, 101900, 101900, 97900]




# Thermristor B-equation (T in Kelvin)
# R = R_N * exp(-B ( 1/T_N - 1/T ))
# T = B / ln (R / r_inf)
# with r_inf = R_N * exp(-B / T_N)
# T_N = 25 deg Celsius
# R_N = 100k Ohm

# Steinhard-Hart equation
# 1/T = a + b ln(R) + c (ln(R))**3
# or
# R = exp( 3root( y - x/2 ) - 3root( y + x/2 ))
# x = 1/c ( a - 1/T)
# y = sqrt( (b/(3*c))**3 + (x/2)**2 )

# equals B-equation when
# a=(1/T_0)-(1/B)*ln(R_0)
# b=1/B
# c=0

# LSQ     Ah=y    h=(A^T A)^-1 A^T y
# WLSQ    Ah=y    h=(A^T W A)^-1 A^T W y

# Solve Steinhard-Hart with LSQ using this gls

# [1/T_i]       [ 1   ln(R_i) ln**3(R_i) ]  [ a ]
# [  .  ]       [ .      .          .    ]  [ b ]
# [  .  ]   =   [ .      .          .    ]  [ c ]
# [  .  ]       [ .      .          .    ]



# raw -> R
# raw = R/(R+100k) * 1023
# <=>  raw*R + raw*100k = R * 1023
# <=>  raw * 100k = R ( 1023 - raw )
# <=>  R = (raw * 100k) / ( 1023 - raw )
def raw2R(raw, R_ref):
    return (raw * R_ref) / (1023 - raw)

# Solve for one temperature sensor
# Input: Measurements     [t]     [d]
#                         [.]     [ ]
#                         [.]     [ ]
def lsq(temperatures, raws, weights, R_ref):
    assert len(temperatures) == len(raws)
    R = np.array([raw2R(r, R_ref) for r in raws])
    Tinv = np.array([1/(t+273.15) for t in temperatures])

    # W = Phi^-1
    # P Phi P^T = I
    # KovarMatrix = sigma^2 (A^T W A)^-1
    W = np.diag(weights)
    #print W

    A = np.zeros((len(R), 3))
    for i, r in enumerate(R):
        A[i, 0] = 1
        A[i, 1] = np.log(r)
        A[i, 2] = A[i, 1] ** 3

    h = np.linalg.lstsq(A, Tinv)
    tmp = np.matmul(A.transpose(), W)
    tmp2 = np.linalg.inv(np.matmul(tmp, A))
    tmp3 = np.matmul(np.matmul(tmp2, A.transpose()), W)
    h2 = np.dot(tmp3, Tinv)

    # print h[0]
    # print h2
    # assert h[0] == h2
    return h2
    pass


def plot(a, b, c):
    ts = np.arange(-20., 30., 1.)
    ts = ts + 273.15
    # x = 1/c ( a - 1/T)
    x = 1./c * ( a - 1./ts)
    # y = sqrt( (b/(3*c))**3 + (x/2)**2 )
    y = np.sqrt( (b/(3*c))**3 + (x/2)**2 )
    # R = exp( 3root( y - x/2 ) - 3root( y + x/2 ))
    R = np.exp( ( y - x/2 )**(1./3) -( y + x/2 )**(1./3) )

    plt.plot(x, y, 'r')


def plot2(a, b, c, R_ref):
    raws = np.arange(100., 900., 1.)
    Rs = raw2R(raws, R_ref)

    Tinv = a + b * np.log(Rs) + c * (np.log(Rs))**3
    Ts = (1/Tinv) - 273.15
    plt.plot(raws, Ts, 'r')



coefficients = [lsq(temps, sr, weights, R_ref) for sr, R_ref in zip(sensors_raws, onehundreds_at0deg)]

print coefficients

for cs, sr, R_ref in zip(coefficients, sensors_raws, onehundreds_at0deg):
    plot2(cs[0], cs[1], cs[2], R_ref=R_ref)
    plt.plot(sr, temps, 'o', label='Original data', markersize=10)

plt.show()

