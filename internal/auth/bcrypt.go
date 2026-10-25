package auth

import "golang.org/x/crypto/bcrypt"

func bcryptHash(s string) (string, error) {
	b, err := bcrypt.GenerateFromPassword([]byte(s), bcrypt.DefaultCost)
	if err != nil {
		return "", err
	}
	return string(b), nil
}
